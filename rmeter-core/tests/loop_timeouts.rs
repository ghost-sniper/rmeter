use rmeter_core::{
    context::UserContext,
    controller::{
        Condition, CursorStep, ExecutionCursor, IfController, LoopController, WhileController,
    },
    engine::Engine,
    event::{RunEvent, SampleEvent},
    plan::{ErrorPolicy, LoopCount, Node, TestPlan, ThreadGroup},
    sampler::{SampleError, SampleResult, Sampler},
};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime},
};

enum Sample {
    Wait(&'static str, u64),
    Check(&'static str),
    Pending(Arc<AtomicBool>),
}
struct Dropped(Arc<AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
impl Sampler for Sample {
    async fn sample(&self, context: &mut UserContext) -> Result<SampleResult, SampleError> {
        let started_at = SystemTime::now();
        let started = tokio::time::Instant::now();
        let label = match self {
            Self::Wait(label, ms) => {
                tokio::time::sleep(Duration::from_millis(*ms)).await;
                context.variables.insert((*label).into(), "done".into());
                *label
            }
            Self::Check(key) => {
                assert!(context.variables.contains_key(*key));
                assert!(!context.last_sample_ok);
                "after"
            }
            Self::Pending(dropped) => {
                let _guard = Dropped(dropped.clone());
                std::future::pending::<()>().await;
                unreachable!()
            }
        };
        Ok(SampleResult {
            label: label.into(),
            started_at,
            elapsed: started.elapsed(),
            success: true,
            response_code: None,
            response_message: None,
            bytes_sent: 0,
            bytes_received: 0,
            response_body: None,
            assertions: vec![],
        })
    }
}
fn wait(label: &'static str, ms: u64) -> Node<Sample> {
    Node::Sample(Sample::Wait(label, ms))
}
fn looping(name: &str, count: u64, timeout: u64, children: Vec<Node<Sample>>) -> Node<Sample> {
    let mut controller = LoopController::new(name, LoopCount::Finite(count), children);
    controller.timeout = Duration::from_millis(timeout);
    Node::LoopController(controller)
}
fn group(children: Vec<Node<Sample>>) -> ThreadGroup<Sample> {
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
fn plan(groups: Vec<ThreadGroup<Sample>>) -> TestPlan<Sample> {
    TestPlan {
        name: "timeouts".into(),
        variables: HashMap::new(),
        groups,
    }
}
async fn collect(plan: TestPlan<Sample>) -> Vec<SampleEvent> {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let task = tokio::spawn(async move { Engine::new(plan).run(tx).await });
    let mut samples = vec![];
    while let Some(event) = rx.recv().await {
        if let RunEvent::SampleCompleted(event) = event {
            samples.push(*event);
        }
    }
    task.await.unwrap().unwrap();
    samples
}
fn labels(samples: &[SampleEvent]) -> Vec<&str> {
    samples.iter().map(|s| s.result.label.as_str()).collect()
}

#[tokio::test(start_paused = true)]
async fn each_round_resets_and_zero_and_legacy_loops_are_unlimited() {
    let events = collect(plan(vec![group(vec![
        looping("reset", 3, 50, vec![wait("round", 40)]),
        looping("unlimited", 2, 0, vec![wait("long", 1000)]),
        Node::Loop {
            count: LoopCount::Finite(1),
            children: vec![wait("legacy", 1000)],
        },
    ])]))
    .await;
    assert_eq!(
        labels(&events),
        ["round", "round", "round", "long", "long", "legacy"]
    );
    assert!(events.iter().all(|e| e.result.success));
}

#[tokio::test(start_paused = true)]
async fn cumulative_round_timeout_keeps_completed_children_and_variables() {
    let events = collect(plan(vec![group(vec![
        looping(
            "limited",
            10,
            50,
            vec![
                wait("saved", 30),
                wait("interrupted", 30),
                wait("skipped", 0),
            ],
        ),
        Node::Sample(Sample::Check("saved")),
    ])]))
    .await;
    assert_eq!(labels(&events), ["saved", "limited", "after"]);
    let failure = &events[1].result;
    assert!(!failure.success);
    assert_eq!(failure.response_code.as_deref(), Some("ControllerTimeout"));
    assert_eq!(failure.elapsed, Duration::from_millis(50));
    assert!(
        failure
            .response_message
            .as_ref()
            .unwrap()
            .contains("iteration 1")
    );
}

#[tokio::test(start_paused = true)]
async fn nested_timeouts_keep_ancestor_budget_and_outer_wins_ties() {
    let events = collect(plan(vec![group(vec![
        looping(
            "outer",
            1,
            100,
            vec![
                looping("inner", 1, 20, vec![wait("slow", 50)]),
                wait("remaining", 90),
            ],
        ),
        wait("after", 0),
    ])]))
    .await;
    assert_eq!(labels(&events), ["inner", "outer", "after"]);
    assert_eq!(events[1].result.elapsed, Duration::from_millis(100));
    for inner_limit in [20, 100] {
        let events = collect(plan(vec![group(vec![looping(
            "outer",
            1,
            20,
            vec![looping("inner", 1, inner_limit, vec![wait("slow", 50)])],
        )])]))
        .await;
        assert_eq!(labels(&events), ["outer"]);
    }
}

#[tokio::test(start_paused = true)]
async fn while_timeout_skips_false_conditions_and_cancels_pending_work() {
    let dropped = Arc::new(AtomicBool::new(false));
    let mut controller = WhileController::new(
        "while",
        true,
        vec![Node::Sample(Sample::Pending(dropped.clone()))],
    );
    controller.timeout = Duration::from_millis(30);
    let mut skipped = WhileController::new("skipped", false, vec![wait("bad", 1000)]);
    skipped.timeout = Duration::from_millis(1);
    let events = collect(plan(vec![group(vec![
        looping("zero", 0, 1, vec![wait("bad", 1000)]),
        Node::While(skipped),
        Node::While(controller),
        wait("after", 0),
    ])]))
    .await;
    assert_eq!(labels(&events), ["while", "after"]);
    assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn error_policies_and_users_have_independent_budgets() {
    for (policy, expected) in [
        (
            ErrorPolicy::Continue,
            vec!["timeout", "after", "timeout", "after"],
        ),
        (ErrorPolicy::NextIteration, vec!["timeout", "timeout"]),
        (ErrorPolicy::StopUser, vec!["timeout"]),
        (ErrorPolicy::StopTest, vec!["timeout"]),
    ] {
        let mut g = group(vec![
            looping("timeout", 5, 10, vec![wait("slow", 100)]),
            wait("after", 0),
        ]);
        g.iterations = LoopCount::Finite(2);
        g.on_error = policy;
        assert_eq!(labels(&collect(plan(vec![g])).await), expected);
    }
    let mut g = group(vec![looping("timeout", 1, 10, vec![wait("slow", 100)])]);
    g.users = 3;
    g.ramp_up = Duration::from_millis(90);
    let mut events = collect(plan(vec![g])).await;
    events.sort_by_key(|e| e.user_index);
    assert_eq!(
        events.iter().map(|e| e.user_index).collect::<Vec<_>>(),
        [0, 1, 2]
    );
    assert!(
        events
            .iter()
            .all(|e| e.result.elapsed == Duration::from_millis(10))
    );
}

#[tokio::test(start_paused = true)]
async fn reporting_backpressure_does_not_consume_loop_budget() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(1);
    let task = tokio::spawn(async move {
        Engine::new(plan(vec![group(vec![looping(
            "limit",
            1,
            50,
            vec![wait("first", 10), wait("second", 10), wait("third", 10)],
        )])]))
        .run(tx)
        .await
    });
    // Leave the channel full with Started, forcing the first sample send to wait.
    tokio::time::sleep(Duration::from_secs(1)).await;
    let mut events = vec![];
    while let Some(event) = rx.recv().await {
        if let RunEvent::SampleCompleted(event) = event {
            events.push(*event);
        }
    }
    task.await.unwrap().unwrap();
    assert_eq!(labels(&events), ["first", "second", "third"]);
}

#[tokio::test(start_paused = true)]
async fn group_deadline_wins_at_the_same_instant_and_drop_cancels_users() {
    let dropped = Arc::new(AtomicBool::new(false));
    let mut g = group(vec![looping(
        "timeout",
        1,
        10,
        vec![Node::Sample(Sample::Pending(dropped.clone()))],
    )]);
    g.max_duration = Some(Duration::from_millis(10));
    assert!(collect(plan(vec![g])).await.is_empty());
    assert!(dropped.load(Ordering::SeqCst));
    let dropped = Arc::new(AtomicBool::new(false));
    let engine = Engine::new(plan(vec![group(vec![looping(
        "timeout",
        1,
        100,
        vec![Node::Sample(Sample::Pending(dropped.clone()))],
    )])]));
    let (tx, _rx) = tokio::sync::mpsc::channel(8);
    let task = tokio::spawn(async move { engine.run(tx).await });
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
    task.abort();
    let _ = task.await;
    tokio::task::yield_now().await;
    assert!(dropped.load(Ordering::SeqCst));
}

#[tokio::test(start_paused = true)]
async fn control_only_loops_timeout_cooperatively_and_while_rounds_reset() {
    let mut controller = WhileController::new(
        "control",
        true,
        vec![Node::If(IfController::new(
            "skip",
            Condition::Constant(false),
            vec![wait("never", 0)],
        ))],
    );
    controller.timeout = Duration::ZERO;
    let nodes = vec![looping(
        "outer control",
        1,
        10,
        vec![Node::While(controller)],
    )];
    let mut cursor = ExecutionCursor::new(&nodes);
    let mut context = UserContext::new("group", 0);
    assert!(matches!(
        cursor.step(&mut context).unwrap(),
        CursorStep::Yield
    ));
    tokio::time::advance(Duration::from_millis(11)).await;
    // The outer iteration includes the infinite, sample-free inner traversal.
    assert!(matches!(
        cursor.step(&mut context).unwrap(),
        CursorStep::Timeout(_)
    ));
    let mut controller = WhileController::new("rounds", true, vec![wait("body", 0)]);
    controller.timeout = Duration::from_millis(50);
    let nodes = vec![Node::While(controller)];
    let mut cursor = ExecutionCursor::new(&nodes);
    for _ in 0..3 {
        assert!(matches!(
            cursor.step(&mut context).unwrap(),
            CursorStep::Sample(_)
        ));
        tokio::time::advance(Duration::from_millis(40)).await;
    }
}
