//! Tokio execution of shared plans with one task and context per virtual user.
//!
//! Drain the event channel concurrently. Sample failures arrive through events;
//! [`EngineError`] represents infrastructure or control-flow failures.

use std::{fmt, future::pending, sync::Arc, time::SystemTime};

use tokio::{
    sync::{mpsc, watch},
    task::{JoinError, JoinSet},
    time::{Instant, sleep_until},
};

use crate::{
    context::UserContext,
    controller::{ControllerError, ControllerTimeout, CursorStep, ExecutionCursor},
    event::{RunEvent, SampleEvent},
    plan::{ErrorPolicy, LoopCount, TestPlan},
    sampler::{SampleResult, Sampler},
};

/// Runs a shared plan with an independent context and task for each virtual user.
pub struct Engine<S> {
    plan: Arc<TestPlan<S>>,
}

/// Infrastructure failures; sampler failures are reported as unsuccessful samples.
#[derive(Debug)]
pub enum EngineError {
    EventsClosed,
    UserTaskFailed(JoinError),
    InvalidDuration {
        group_name: String,
    },
    ControllerFailed {
        group_name: String,
        user_index: usize,
        error: ControllerError,
    },
}

impl fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EventsClosed => formatter.write_str("run event receiver was closed"),
            Self::UserTaskFailed(error) => write!(formatter, "virtual user task failed: {error}"),
            Self::InvalidDuration { group_name } => {
                write!(
                    formatter,
                    "duration exceeds the clock range for group {group_name:?}"
                )
            }
            Self::ControllerFailed {
                group_name,
                user_index,
                error,
            } => {
                write!(
                    formatter,
                    "group {group_name:?}, user {user_index}: {error}"
                )
            }
        }
    }
}

impl std::error::Error for EngineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::UserTaskFailed(error) => Some(error),
            Self::ControllerFailed { error, .. } => Some(error),
            _ => None,
        }
    }
}

impl<S: Sampler + 'static> Engine<S> {
    /// Own a plan for repeated, independent calls to [`Self::run`].
    pub fn new(plan: TestPlan<S>) -> Self {
        Self {
            plan: Arc::new(plan),
        }
    }

    /// Sends Started, completed samples, then Finished after all users have stopped.
    /// Consume the bounded event channel concurrently to avoid blocking the run.
    /// User and iteration indices are zero-based. Each call starts a fresh run.
    /// Dropping this future aborts its user tasks; StopTest cancels all groups.
    pub async fn run(&self, events: mpsc::Sender<RunEvent>) -> Result<(), EngineError> {
        let function_environment = rmeter_function::Environment::default();
        let started = Instant::now();
        let schedules = self
            .plan
            .groups
            .iter()
            .map(|group| {
                let invalid_duration = || EngineError::InvalidDuration {
                    group_name: group.name.clone(),
                };
                let ramp_end = started
                    .checked_add(group.ramp_up)
                    .ok_or_else(invalid_duration)?;
                let deadline = group
                    .max_duration
                    .map(|duration| started.checked_add(duration).ok_or_else(invalid_duration))
                    .transpose()?;
                Ok((ramp_end, deadline))
            })
            .collect::<Result<Vec<_>, EngineError>>()?;

        events
            .send(RunEvent::Started {
                at: SystemTime::now(),
            })
            .await
            .map_err(|_| EngineError::EventsClosed)?;

        let (stop, _) = watch::channel(false);
        let mut users = JoinSet::new();
        for (group_index, group) in self.plan.groups.iter().enumerate() {
            let (ramp_end, deadline) = schedules[group_index];
            for user_index in 0..group.users {
                // The first user starts immediately; subsequent users are spaced
                // evenly at ramp_up / users, as in a conventional thread group.
                let delay = (ramp_end - started).mul_f64(user_index as f64 / group.users as f64);
                users.spawn(run_user(
                    Arc::clone(&self.plan),
                    group_index,
                    user_index,
                    (started + delay, deadline),
                    events.clone(),
                    stop.clone(),
                    function_environment.clone(),
                ));
            }
        }

        let mut failure = None;
        while let Some(result) = users.join_next().await {
            let error = match result {
                Ok(Ok(())) => continue,
                Ok(Err(error)) => error,
                Err(error) => EngineError::UserTaskFailed(error),
            };
            if failure.is_none() {
                failure = Some(error);
                users.abort_all();
            }
        }

        let finished = events
            .send(RunEvent::Finished {
                at: SystemTime::now(),
            })
            .await
            .map_err(|_| EngineError::EventsClosed);
        match failure {
            Some(error) => Err(error),
            None => finished,
        }
    }
}

async fn run_user<S: Sampler>(
    plan: Arc<TestPlan<S>>,
    group_index: usize,
    user_index: usize,
    schedule: (Instant, Option<Instant>),
    events: mpsc::Sender<RunEvent>,
    stop: watch::Sender<bool>,
    function_environment: rmeter_function::Environment,
) -> Result<(), EngineError> {
    let (start_at, deadline) = schedule;
    let mut cancelled = stop.subscribe();
    if *cancelled.borrow() {
        return Ok(());
    }

    // Cancellation also interrupts ramp-up waits, pending samplers, and blocked
    // event sends. The samplers' futures must therefore be safe to drop.
    tokio::select! {
        biased;
        _ = cancelled.wait_for(|stopped| *stopped) => Ok(()),
        _ = events.closed() => Err(EngineError::EventsClosed),
        _ = wait_for_deadline(deadline) => Ok(()),
        result = async {
            sleep_until(start_at).await;
            execute_user(&plan, group_index, user_index, &events, &stop, function_environment).await
        } => result,
    }
}

async fn wait_for_deadline(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => sleep_until(deadline).await,
        None => pending().await,
    }
}

async fn wait_for_controller(remaining: Option<std::time::Duration>) {
    match remaining {
        Some(remaining) => tokio::time::sleep(remaining).await,
        None => pending().await,
    }
}

fn timeout_result(timeout: ControllerTimeout) -> SampleResult {
    SampleResult {
        label: timeout.controller_name,
        started_at: timeout.started_at,
        elapsed: timeout.elapsed,
        success: false,
        response_code: Some("ControllerTimeout".into()),
        response_message: Some(format!(
            "controller iteration {} exceeded its {} ms timeout",
            timeout.iteration.saturating_add(1),
            timeout.timeout.as_millis()
        )),
        bytes_sent: 0,
        bytes_received: 0,
        response_body: None,
        assertions: Vec::new(),
    }
}

async fn execute_user<S: Sampler>(
    plan: &TestPlan<S>,
    group_index: usize,
    user_index: usize,
    events: &mpsc::Sender<RunEvent>,
    stop: &watch::Sender<bool>,
    function_environment: rmeter_function::Environment,
) -> Result<(), EngineError> {
    let group = &plan.groups[group_index];
    let mut context = UserContext::new(&group.name, user_index);
    context.function_environment = function_environment;
    context.variables.clone_from(&plan.variables);
    let mut cursor = ExecutionCursor::new(&group.children);
    let mut remaining = group.iterations;

    while remaining != LoopCount::Finite(0) {
        cursor.reset();
        loop {
            let step =
                cursor
                    .step(&mut context)
                    .map_err(|error| EngineError::ControllerFailed {
                        group_name: group.name.clone(),
                        user_index,
                        error,
                    })?;
            let result = match step {
                CursorStep::Yield => {
                    tokio::task::yield_now().await;
                    continue;
                }
                CursorStep::Done => break,
                CursorStep::Timeout(timeout) => {
                    context.script_logs = Default::default();
                    timeout_result(timeout)
                }
                CursorStep::Sample(sampler) => {
                    let started_at = SystemTime::now();
                    let started = Instant::now();
                    context.script_logs = Default::default();
                    let sampled = tokio::select! {
                        biased;
                        _ = wait_for_controller(cursor.timeout_remaining()) => None,
                        result = sampler.sample(&mut context) => Some(result),
                    };
                    if let Some(timeout) = cursor.take_timeout() {
                        timeout_result(timeout)
                    } else {
                        match sampled.expect("a controller timer only wakes at an active deadline")
                        {
                            Ok(result) => result,
                            Err(error) => SampleResult {
                                label: "sampler error".into(),
                                started_at,
                                elapsed: started.elapsed(),
                                success: false,
                                response_code: Some(format!("{:?}", error.kind)),
                                response_message: Some(error.message),
                                bytes_sent: 0,
                                bytes_received: 0,
                                response_body: None,
                                assertions: Vec::new(),
                            },
                        }
                    }
                }
            };
            context.last_sample_ok = result.success;
            let reporting_started = Instant::now();
            events
                .send(RunEvent::SampleCompleted(Box::new(SampleEvent::new(
                    &context, result,
                ))))
                .await
                .map_err(|_| EngineError::EventsClosed)?;
            cursor.exclude_reporting_time(reporting_started.elapsed());

            if !context.last_sample_ok {
                match group.on_error {
                    ErrorPolicy::Continue => {}
                    ErrorPolicy::NextIteration => break,
                    ErrorPolicy::StopUser => return Ok(()),
                    ErrorPolicy::StopTest => {
                        stop.send_replace(true);
                        return Ok(());
                    }
                }
            }

            // Ready samplers and channel sends may never yield on their own.
            // Keep infinite loops responsive to other users and cancellation.
            tokio::task::yield_now().await;
        }

        if !cursor.has_work() {
            break;
        }
        if let LoopCount::Finite(count) = &mut remaining {
            *count -= 1;
        }
        let Some(iteration) = context.iteration.checked_add(1) else {
            break;
        };
        context.iteration = iteration;
        // NextIteration can skip the yield inside the sampler loop.
        tokio::task::yield_now().await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, time::Duration};

    use tokio::sync::Barrier;

    use super::*;
    use crate::{
        plan::{Node, ThreadGroup},
        sampler::{SampleError, SampleErrorKind},
    };

    enum TestSampler {
        Sample(&'static str, bool),
        Error,
        Pending,
        Delay(Duration),
        Context,
        Barrier(Arc<Barrier>),
        Panic,
    }

    impl Sampler for TestSampler {
        async fn sample(&self, context: &mut UserContext) -> Result<SampleResult, SampleError> {
            let started_at = SystemTime::now();
            let started = Instant::now();
            let (label, success) = match self {
                Self::Sample(label, success) => ((*label).to_owned(), *success),
                Self::Error => {
                    return Err(SampleError {
                        kind: SampleErrorKind::Transport,
                        message: "connection failed".into(),
                    });
                }
                Self::Pending => pending().await,
                Self::Delay(duration) => {
                    tokio::time::sleep(*duration).await;
                    ("delayed".into(), true)
                }
                Self::Context => {
                    assert_eq!(context.variables.get("seed").unwrap(), "value");
                    let calls = context
                        .variables
                        .entry("calls".into())
                        .or_insert_with(|| "0".into());
                    let count = calls.parse::<usize>().unwrap() + 1;
                    *calls = count.to_string();
                    (format!("{count}:{}", context.last_sample_ok), true)
                }
                Self::Barrier(barrier) => {
                    barrier.wait().await;
                    ("concurrent".into(), true)
                }
                Self::Panic => panic!("sampler panicked"),
            };
            Ok(SampleResult {
                label,
                started_at,
                elapsed: started.elapsed(),
                success,
                response_code: None,
                response_message: None,
                bytes_sent: 1,
                bytes_received: 2,
                response_body: None,
                assertions: Vec::new(),
            })
        }
    }

    fn sample(label: &'static str) -> Node<TestSampler> {
        Node::Sample(TestSampler::Sample(label, true))
    }

    fn group(children: Vec<Node<TestSampler>>) -> ThreadGroup<TestSampler> {
        ThreadGroup {
            name: "group".into(),
            users: 1,
            ramp_up: Duration::ZERO,
            iterations: LoopCount::Finite(1),
            max_duration: None,
            on_error: ErrorPolicy::Continue,
            children,
        }
    }

    fn plan(groups: Vec<ThreadGroup<TestSampler>>) -> TestPlan<TestSampler> {
        TestPlan {
            name: "test".into(),
            variables: HashMap::from([("seed".into(), "value".into())]),
            groups,
        }
    }

    async fn collect(plan: TestPlan<TestSampler>) -> (Result<(), EngineError>, Vec<RunEvent>) {
        // Capacity one also exercises backpressure and concurrent consumption.
        let (sender, mut receiver) = mpsc::channel(1);
        let task = tokio::spawn(async move { Engine::new(plan).run(sender).await });
        let mut events = Vec::new();
        while let Some(event) = receiver.recv().await {
            events.push(event);
        }
        (task.await.unwrap(), events)
    }

    fn samples(events: &[RunEvent]) -> Vec<&SampleEvent> {
        assert!(matches!(events.first(), Some(RunEvent::Started { .. })));
        assert!(matches!(events.last(), Some(RunEvent::Finished { .. })));
        events
            .iter()
            .filter_map(|event| match event {
                RunEvent::SampleCompleted(sample) => Some(sample.as_ref()),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn executes_nested_nodes_for_each_group_iteration() {
        let mut group = group(vec![
            sample("first"),
            Node::Loop {
                count: LoopCount::Finite(2),
                children: vec![Node::Sequence(vec![sample("nested")])],
            },
        ]);
        group.iterations = LoopCount::Finite(2);
        let (result, events) = collect(plan(vec![group])).await;
        result.unwrap();

        let actual: Vec<_> = samples(&events)
            .iter()
            .map(|sample| {
                (
                    sample.result.label.as_str(),
                    sample.iteration,
                    sample.user_index,
                )
            })
            .collect();
        assert_eq!(
            actual,
            vec![
                ("first", 0, 0),
                ("nested", 0, 0),
                ("nested", 0, 0),
                ("first", 1, 0),
                ("nested", 1, 0),
                ("nested", 1, 0),
            ]
        );
    }

    #[tokio::test]
    async fn contexts_are_independent_and_persist_across_iterations() {
        let mut group = group(vec![
            Node::Sample(TestSampler::Sample("failure", false)),
            Node::Sample(TestSampler::Context),
        ]);
        group.users = 2;
        group.iterations = LoopCount::Finite(2);
        let (result, events) = collect(plan(vec![group])).await;
        result.unwrap();
        let samples = samples(&events);
        for user_index in 0..2 {
            let labels: Vec<_> = samples
                .iter()
                .filter(|sample| sample.user_index == user_index)
                .map(|sample| sample.result.label.as_str())
                .collect();
            assert_eq!(labels, vec!["failure", "1:false", "failure", "2:false"]);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn groups_and_users_run_concurrently() {
        let barrier = Arc::new(Barrier::new(3));
        let mut first = group(vec![Node::Sample(TestSampler::Barrier(barrier.clone()))]);
        first.users = 2;
        let mut second = group(vec![Node::Sample(TestSampler::Barrier(barrier))]);
        second.name = "second".into();
        let (result, events) =
            tokio::time::timeout(Duration::from_secs(1), collect(plan(vec![first, second])))
                .await
                .expect("users must reach the barrier concurrently");
        result.unwrap();
        let samples = samples(&events);
        assert_eq!(samples.len(), 3);
        assert!(samples.iter().any(|sample| sample.group_name == "second"));
    }

    #[tokio::test]
    async fn applies_all_error_policies_to_failed_results_and_sampler_errors() {
        for sampler_error in [false, true] {
            for (policy, expected) in [
                (ErrorPolicy::Continue, vec![0, 0, 1, 1]),
                (ErrorPolicy::NextIteration, vec![0, 1]),
                (ErrorPolicy::StopUser, vec![0]),
                (ErrorPolicy::StopTest, vec![0]),
            ] {
                let failure = if sampler_error {
                    TestSampler::Error
                } else {
                    TestSampler::Sample("failure", false)
                };
                let mut group = group(vec![Node::Sample(failure), sample("after")]);
                group.on_error = policy;
                group.iterations = LoopCount::Finite(2);
                let (result, events) = collect(plan(vec![group])).await;
                result.unwrap();
                let samples = samples(&events);
                assert_eq!(
                    samples
                        .iter()
                        .map(|sample| sample.iteration)
                        .collect::<Vec<_>>(),
                    expected,
                    "{policy:?}, sampler_error={sampler_error}"
                );
                assert!(!samples[0].result.success);
                if sampler_error {
                    assert_eq!(
                        samples[0].result.response_code.as_deref(),
                        Some("Transport")
                    );
                    assert_eq!(
                        samples[0].result.response_message.as_deref(),
                        Some("connection failed")
                    );
                }
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn stop_test_interrupts_pending_users_and_ramp_up_in_other_groups() {
        let mut stopped = group(vec![
            Node::Sample(TestSampler::Delay(Duration::from_secs(1))),
            Node::Sample(TestSampler::Sample("stop", false)),
        ]);
        stopped.on_error = ErrorPolicy::StopTest;
        let mut waiting = group(vec![Node::Sample(TestSampler::Pending)]);
        waiting.users = 2;
        waiting.ramp_up = Duration::from_secs(100);
        let started = Instant::now();
        let (result, events) = tokio::time::timeout(
            Duration::from_secs(2),
            collect(plan(vec![waiting, stopped])),
        )
        .await
        .expect("StopTest must interrupt other groups");
        result.unwrap();
        assert_eq!(samples(&events).len(), 2);
        assert_eq!(started.elapsed(), Duration::from_secs(1));
    }

    #[tokio::test]
    async fn stop_user_allows_other_users_to_finish() {
        let mut stopped = group(vec![Node::Sample(TestSampler::Error), sample("skipped")]);
        stopped.on_error = ErrorPolicy::StopUser;
        let other = group(vec![sample("other")]);
        let (result, events) = collect(plan(vec![stopped, other])).await;
        result.unwrap();
        let samples = samples(&events);
        assert_eq!(samples.len(), 2);
        assert!(samples.iter().any(|sample| sample.result.label == "other"));
    }

    #[tokio::test(start_paused = true)]
    async fn ramp_up_spaces_user_starts_evenly() {
        let mut group = group(vec![sample("ramp")]);
        group.users = 3;
        group.ramp_up = Duration::from_secs(3);
        let (sender, mut receiver) = mpsc::channel(1);
        let started = Instant::now();
        let task = tokio::spawn(async move { Engine::new(plan(vec![group])).run(sender).await });
        let mut starts = Vec::new();
        while let Some(event) = receiver.recv().await {
            if let RunEvent::SampleCompleted(sample) = event {
                starts.push((sample.user_index, started.elapsed()));
            }
        }
        task.await.unwrap().unwrap();
        assert_eq!(
            starts,
            vec![
                (0, Duration::ZERO),
                (1, Duration::from_secs(1)),
                (2, Duration::from_secs(2)),
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn deadline_includes_ramp_up_and_interrupts_pending_samples() {
        let mut group = group(vec![Node::Sample(TestSampler::Pending)]);
        group.users = 2;
        group.ramp_up = Duration::from_secs(20);
        group.max_duration = Some(Duration::from_secs(2));
        let started = Instant::now();
        let (result, events) = collect(plan(vec![group])).await;
        result.unwrap();
        assert!(samples(&events).is_empty());
        assert_eq!(started.elapsed(), Duration::from_secs(2));
    }

    #[tokio::test(start_paused = true)]
    async fn deadline_stops_infinite_group_and_node_loops() {
        for nested in [false, true] {
            let child = Node::Sample(TestSampler::Delay(Duration::from_secs(1)));
            let mut group = group(vec![if nested {
                Node::Loop {
                    count: LoopCount::Infinite,
                    children: vec![child],
                }
            } else {
                child
            }]);
            group.iterations = LoopCount::Infinite;
            group.max_duration = Some(Duration::from_millis(2500));
            let started = Instant::now();
            let (result, events) = collect(plan(vec![group])).await;
            result.unwrap();
            let samples = samples(&events);
            assert_eq!(samples.len(), 2);
            assert_eq!(samples[1].iteration, if nested { 0 } else { 1 });
            assert_eq!(started.elapsed(), Duration::from_millis(2500));
        }
    }

    #[tokio::test(start_paused = true)]
    async fn empty_and_disabled_groups_finish_without_spinning() {
        let mut empty = group(vec![Node::Loop {
            count: LoopCount::Infinite,
            children: vec![],
        }]);
        empty.iterations = LoopCount::Infinite;
        let mut no_users = group(vec![Node::Sample(TestSampler::Pending)]);
        no_users.users = 0;
        let mut no_iterations = group(vec![Node::Sample(TestSampler::Pending)]);
        no_iterations.iterations = LoopCount::Finite(0);
        let mut no_duration = group(vec![Node::Sample(TestSampler::Pending)]);
        no_duration.max_duration = Some(Duration::ZERO);
        for groups in [vec![], vec![empty, no_users, no_iterations, no_duration]] {
            let (result, events) =
                tokio::time::timeout(Duration::from_secs(1), collect(plan(groups)))
                    .await
                    .expect("sample-free plans must terminate");
            result.unwrap();
            assert!(samples(&events).is_empty());
        }
    }

    #[tokio::test(start_paused = true)]
    async fn receiver_closure_stops_pending_users() {
        let (sender, mut receiver) = mpsc::channel(1);
        let task = tokio::spawn(async move {
            Engine::new(plan(vec![group(vec![Node::Sample(TestSampler::Pending)])]))
                .run(sender)
                .await
        });
        assert!(matches!(
            receiver.recv().await,
            Some(RunEvent::Started { .. })
        ));
        tokio::task::yield_now().await;
        drop(receiver);
        let result = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("receiver closure must cancel pending samplers")
            .unwrap();
        assert!(matches!(result, Err(EngineError::EventsClosed)));
    }

    #[tokio::test(start_paused = true)]
    async fn sampler_panic_is_reported_and_other_users_are_aborted() {
        let (result, events) = tokio::time::timeout(
            Duration::from_secs(1),
            collect(plan(vec![
                group(vec![Node::Sample(TestSampler::Pending)]),
                group(vec![Node::Sample(TestSampler::Panic)]),
            ])),
        )
        .await
        .expect("panics must not leave pending users running");
        assert!(matches!(result, Err(EngineError::UserTaskFailed(error)) if error.is_panic()));
        assert!(samples(&events).is_empty());
    }
}
