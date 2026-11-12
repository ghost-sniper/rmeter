use std::{
    collections::HashMap,
    time::{Duration, SystemTime},
};

use rmeter_core::{
    context::UserContext,
    controller::{
        CompareOperator, Condition, ControllerError, CursorStep, ExecutionCursor,
        GenericController, IfController, Operand, WhileController,
    },
    engine::{Engine, EngineError},
    event::{RunEvent, SampleEvent},
    plan::{ErrorPolicy, LoopCount, Node, TestPlan, ThreadGroup},
    sampler::{SampleError, SampleResult, Sampler},
};

fn next<'a, S>(
    cursor: &mut ExecutionCursor<'a, S>,
    context: &mut UserContext,
) -> Result<Option<&'a S>, ControllerError> {
    for _ in 0..100 {
        match cursor.step(context)? {
            CursorStep::Sample(sample) => return Ok(Some(sample)),
            CursorStep::Done => return Ok(None),
            CursorStep::Yield => {}
            CursorStep::Timeout(_) => panic!("unexpected controller timeout"),
        }
    }
    panic!("unexpected unbounded control traversal");
}

fn context() -> UserContext {
    UserContext::new("test", 0)
}

fn compare(left: Operand, operator: CompareOperator, right: &str) -> Condition {
    Condition::Compare {
        left,
        operator,
        right: Operand::Literal(right.into()),
    }
}

#[test]
fn conditions_read_context_short_circuit_and_report_invalid_values() {
    let mut context = context();
    context.iteration = u64::MAX;
    context.variables.insert("enabled".into(), "TRUE".into());
    assert!(
        Condition::VariableBool("enabled".into())
            .evaluate(&mut context)
            .unwrap()
    );
    assert!(
        Condition::from("${JMeterThread.last_sample_ok}")
            .evaluate(&mut context)
            .unwrap()
    );
    assert!(!Condition::from("not true").evaluate(&mut context).unwrap());
    assert!(
        compare(
            Operand::Iteration,
            CompareOperator::Greater,
            "18446744073709551614"
        )
        .evaluate(&mut context)
        .unwrap()
    );
    assert!(
        compare(
            Operand::Iteration,
            CompareOperator::Equal,
            "18446744073709551615"
        )
        .evaluate(&mut context)
        .unwrap()
    );
    assert!(
        compare(Operand::Literal("-1".into()), CompareOperator::Less, "0")
            .evaluate(&mut context)
            .unwrap()
    );
    assert!(
        compare(Operand::UserIndex, CompareOperator::LessOrEqual, "0")
            .evaluate(&mut context)
            .unwrap()
    );
    assert!(
        compare(Operand::UserIndex, CompareOperator::GreaterOrEqual, "0")
            .evaluate(&mut context)
            .unwrap()
    );
    assert!(
        compare(Operand::LastSampleOk, CompareOperator::NotEqual, "false")
            .evaluate(&mut context)
            .unwrap()
    );
    let missing = Condition::VariableBool("missing".into());
    assert!(
        Condition::Any(vec![true.into(), missing.clone()])
            .evaluate(&mut context)
            .unwrap()
    );
    assert!(
        !Condition::All(vec![false.into(), missing.clone()])
            .evaluate(&mut context)
            .unwrap()
    );
    assert!(
        Condition::Not(Box::new(false.into()))
            .evaluate(&mut context)
            .unwrap()
    );
    assert!(missing.evaluate(&mut context).is_err());
    assert!(
        compare(Operand::Literal("text".into()), CompareOperator::Less, "1")
            .evaluate(&mut context)
            .is_err()
    );
    context.variables.insert("enabled".into(), "maybe".into());
    assert!(
        Condition::VariableBool("enabled".into())
            .evaluate(&mut context)
            .is_err()
    );
    assert!(Condition::from("${broken").evaluate(&mut context).is_err());
    assert!(Condition::from("${}").evaluate(&mut context).is_err());
}

#[test]
fn generic_controllers_share_nested_traversal_and_reset() {
    let nodes = vec![
        Node::Controller(GenericController::new(
            "root",
            vec![
                Node::Sample(1),
                Node::Controller(GenericController::new("inner", vec![Node::Sample(2)])),
            ],
        )),
        Node::Sample(3),
    ];
    let mut cursor = ExecutionCursor::new(&nodes);
    let mut context = context();
    for _ in 0..2 {
        for expected in [1, 2, 3] {
            assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&expected));
        }
        assert!(next(&mut cursor, &mut context).unwrap().is_none());
        cursor.reset();
    }
}

#[test]
fn if_entry_mode_executes_the_whole_body_and_rechecks_after_reset() {
    let nodes = vec![
        Node::If(IfController::new(
            "if",
            "${enabled}",
            vec![Node::Sample(1), Node::Sample(2)],
        )),
        Node::Sample(3),
    ];
    let mut cursor = ExecutionCursor::new(&nodes);
    let mut context = context();
    context.variables.insert("enabled".into(), "true".into());
    assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&1));
    context.variables.insert("enabled".into(), "false".into());
    assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&2));
    assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&3));
    cursor.reset();
    assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&3));
    cursor.reset();
    context.variables.insert("enabled".into(), "true".into());
    assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&1));
}

#[test]
fn evaluate_all_exits_nested_infinite_loop_without_removing_future_entries() {
    let mut controller = IfController::new(
        "gate",
        Condition::LastSampleOk,
        vec![Node::Loop {
            count: LoopCount::Infinite,
            children: vec![Node::Sample(1)],
        }],
    );
    controller.evaluate_all = true;
    let nodes = vec![Node::If(controller), Node::Sample(2)];
    let mut cursor = ExecutionCursor::new(&nodes);
    let mut context = context();
    assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&1));
    context.last_sample_ok = false;
    assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&2));
    context.last_sample_ok = true;
    cursor.reset();
    assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&1));
}

#[test]
fn skipped_if_can_execute_on_the_next_parent_loop_iteration() {
    let nodes = vec![Node::Loop {
        count: LoopCount::Finite(2),
        children: vec![
            Node::If(IfController::new(
                "gate",
                "${enabled}",
                vec![Node::Sample(1)],
            )),
            Node::Sample(2),
        ],
    }];
    let mut cursor = ExecutionCursor::new(&nodes);
    let mut context = context();
    context.variables.insert("enabled".into(), "false".into());
    assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&2));
    context.variables.insert("enabled".into(), "true".into());
    assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&1));
    assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&2));
    assert!(next(&mut cursor, &mut context).unwrap().is_none());
}

#[test]
fn while_checks_at_entry_and_iteration_end_not_between_samplers() {
    let nodes = vec![
        Node::While(WhileController::new(
            "while",
            "${enabled}",
            vec![Node::Sample(1), Node::Sample(2)],
        )),
        Node::Sample(3),
    ];
    let mut cursor = ExecutionCursor::new(&nodes);
    let mut context = context();
    context.variables.insert("enabled".into(), "false".into());
    assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&3));
    cursor.reset();
    context.variables.insert("enabled".into(), "true".into());
    assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&1));
    context.variables.insert("enabled".into(), "false".into());
    assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&2));
    assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&3));
}

#[test]
fn while_blank_and_last_have_distinct_entry_behavior() {
    for (condition, expected) in [("", 1), ("LaSt", 2)] {
        let nodes = vec![
            Node::While(WhileController::new(
                "while",
                condition,
                vec![Node::Sample(1)],
            )),
            Node::Sample(2),
        ];
        let mut cursor = ExecutionCursor::new(&nodes);
        let mut context = context();
        context.last_sample_ok = false;
        assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&expected));
        if expected == 1 {
            assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&2));
        }
    }
}

#[test]
fn blank_while_only_uses_the_final_samples_result() {
    let nodes = vec![
        Node::While(WhileController::new(
            "while",
            "",
            vec![Node::Sample(1), Node::Sample(2)],
        )),
        Node::Sample(3),
    ];
    let mut cursor = ExecutionCursor::new(&nodes);
    let mut context = context();
    assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&1));
    context.last_sample_ok = false;
    assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&2));
    context.last_sample_ok = true;
    assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&1));
    assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&2));
    context.last_sample_ok = false;
    assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&3));
}

#[test]
fn while_expression_continues_for_non_false_strings() {
    let nodes = vec![
        Node::While(WhileController::new(
            "while",
            "${value}",
            vec![Node::Sample(1)],
        )),
        Node::Sample(2),
    ];
    let mut cursor = ExecutionCursor::new(&nodes);
    let mut context = context();
    context.variables.insert("value".into(), "anything".into());
    assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&1));
    context.variables.insert("value".into(), "FaLsE".into());
    assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&2));
}

#[test]
fn while_exposes_zero_based_index_and_restarts_in_each_parent_iteration() {
    let condition = compare(
        Operand::Variable("__jm__poll__idx".into()),
        CompareOperator::Less,
        "2",
    );
    let nodes = vec![Node::Loop {
        count: LoopCount::Finite(2),
        children: vec![
            Node::While(WhileController::new(
                "poll",
                condition,
                vec![Node::Sample(1)],
            )),
            Node::Sample(2),
        ],
    }];
    let mut cursor = ExecutionCursor::new(&nodes);
    let mut context = context();
    for _ in 0..2 {
        for index in ["0", "1"] {
            assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&1));
            assert_eq!(context.variables["__jm__poll__idx"], index);
        }
        assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&2));
    }
    assert!(next(&mut cursor, &mut context).unwrap().is_none());
}

#[test]
fn shared_definitions_have_independent_cursor_state_and_contexts() {
    let nodes = vec![
        Node::While(WhileController::new(
            "while",
            "${enabled}",
            vec![Node::Sample(1)],
        )),
        Node::Sample(2),
    ];
    let mut first = ExecutionCursor::new(&nodes);
    let mut second = ExecutionCursor::new(&nodes);
    let mut first_context = context();
    let mut second_context = context();
    first_context
        .variables
        .insert("enabled".into(), "true".into());
    second_context
        .variables
        .insert("enabled".into(), "false".into());
    assert_eq!(next(&mut first, &mut first_context).unwrap(), Some(&1));
    assert_eq!(next(&mut second, &mut second_context).unwrap(), Some(&2));
    assert_eq!(next(&mut first, &mut first_context).unwrap(), Some(&1));
    first.reset();
    assert_eq!(next(&mut first, &mut first_context).unwrap(), Some(&1));
    assert!(next(&mut second, &mut second_context).unwrap().is_none());
}

#[test]
fn condition_only_infinite_loops_yield_and_can_observe_updated_context() {
    let nodes = vec![
        Node::While(WhileController::new(
            "while",
            "${enabled}",
            vec![Node::If(IfController::new(
                "skip",
                false,
                vec![Node::Sample(1)],
            ))],
        )),
        Node::Sample(2),
    ];
    let mut cursor = ExecutionCursor::new(&nodes);
    let mut context = context();
    context.variables.insert("enabled".into(), "true".into());
    assert!(matches!(
        cursor.step(&mut context).unwrap(),
        CursorStep::Yield
    ));
    context.variables.insert("enabled".into(), "false".into());
    assert_eq!(next(&mut cursor, &mut context).unwrap(), Some(&2));
}

#[test]
fn empty_while_finishes_and_condition_errors_name_the_controller() {
    let nodes = vec![Node::While(WhileController::<i32>::new(
        "empty",
        true,
        vec![],
    ))];
    assert!(
        next(&mut ExecutionCursor::new(&nodes), &mut context())
            .unwrap()
            .is_none()
    );
    let nodes = vec![Node::If(IfController::new(
        "bad",
        "${missing}",
        vec![Node::Sample(1)],
    ))];
    let error = next(&mut ExecutionCursor::new(&nodes), &mut context()).unwrap_err();
    assert_eq!(error.controller_name, "bad");
    assert!(error.source.message.contains("missing"));
}

enum TestSampler {
    Sample(&'static str, bool),
    Increment,
    Pending,
}

impl Sampler for TestSampler {
    async fn sample(&self, context: &mut UserContext) -> Result<SampleResult, SampleError> {
        let (label, success) = match self {
            Self::Sample(label, success) => ((*label).into(), *success),
            Self::Increment => {
                let count = context.variables["count"].parse::<u64>().unwrap() + 1;
                context.variables.insert("count".into(), count.to_string());
                (count.to_string(), true)
            }
            Self::Pending => std::future::pending().await,
        };
        Ok(SampleResult {
            label,
            success,
            started_at: SystemTime::now(),
            elapsed: Duration::ZERO,
            response_code: None,
            response_message: None,
            bytes_sent: 0,
            bytes_received: 0,
            response_body: None,
            assertions: Vec::new(),
        })
    }
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
        name: "controllers".into(),
        variables: HashMap::from([("count".into(), "0".into())]),
        groups,
    }
}

async fn run(plan: TestPlan<TestSampler>) -> (Result<(), EngineError>, Vec<RunEvent>) {
    let engine = Engine::new(plan);
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    tokio::join!(engine.run(sender), async {
        let mut events = Vec::new();
        while let Some(event) = receiver.recv().await {
            events.push(event);
        }
        events
    })
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
async fn engine_advances_iterations_even_when_initial_if_iterations_are_skipped() {
    let mut group = group(vec![Node::If(IfController::new(
        "later",
        compare(Operand::Iteration, CompareOperator::GreaterOrEqual, "2"),
        vec![Node::Sample(TestSampler::Sample("ran", true))],
    ))]);
    group.iterations = LoopCount::Finite(4);
    let (result, events) = run(plan(vec![group])).await;
    result.unwrap();
    assert_eq!(
        samples(&events)
            .iter()
            .map(|sample| sample.iteration)
            .collect::<Vec<_>>(),
        vec![2, 3]
    );
}

#[tokio::test]
async fn engine_while_observes_sampler_variable_changes_for_each_user() {
    let mut group = group(vec![Node::While(WhileController::new(
        "count",
        compare(
            Operand::Variable("count".into()),
            CompareOperator::Less,
            "2",
        ),
        vec![Node::Sample(TestSampler::Increment)],
    ))]);
    group.users = 2;
    let (result, events) = run(plan(vec![group])).await;
    result.unwrap();
    let samples = samples(&events);
    assert_eq!(samples.len(), 4);
    for user in 0..2 {
        assert_eq!(
            samples
                .iter()
                .filter(|sample| sample.user_index == user)
                .map(|sample| sample.result.label.as_str())
                .collect::<Vec<_>>(),
            vec!["1", "2"]
        );
    }
}

#[tokio::test]
async fn next_iteration_abandons_and_resets_nested_while_state() {
    let mut group = group(vec![Node::While(WhileController::new(
        "forever",
        true,
        vec![
            Node::Sample(TestSampler::Sample("failed", false)),
            Node::Sample(TestSampler::Sample("skip", true)),
        ],
    ))]);
    group.iterations = LoopCount::Finite(2);
    group.on_error = ErrorPolicy::NextIteration;
    let (result, events) = run(plan(vec![group])).await;
    result.unwrap();
    assert_eq!(
        samples(&events)
            .iter()
            .map(|sample| (sample.result.label.as_str(), sample.iteration))
            .collect::<Vec<_>>(),
        vec![("failed", 0), ("failed", 1)]
    );
}

#[tokio::test]
async fn engine_reports_condition_failure_and_aborts_pending_users() {
    let failed = group(vec![Node::If(IfController::new(
        "bad",
        "${missing}",
        vec![Node::Sample(TestSampler::Increment)],
    ))]);
    let waiting = group(vec![Node::Sample(TestSampler::Pending)]);
    let (result, events) =
        tokio::time::timeout(Duration::from_secs(2), run(plan(vec![waiting, failed])))
            .await
            .expect("controller error must abort pending users");
    let Err(EngineError::ControllerFailed {
        group_name,
        user_index,
        error,
    }) = result
    else {
        panic!("expected controller failure")
    };
    assert_eq!(group_name, "group");
    assert_eq!(user_index, 0);
    assert_eq!(error.controller_name, "bad");
    assert!(samples(&events).is_empty());
}

#[tokio::test(start_paused = true)]
async fn deadline_interrupts_condition_only_infinite_while() {
    let mut group = group(vec![Node::While(WhileController::new(
        "forever",
        true,
        vec![Node::If(IfController::new(
            "skip",
            false,
            vec![Node::Sample(TestSampler::Increment)],
        ))],
    ))]);
    group.max_duration = Some(Duration::from_secs(1));
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let task = tokio::spawn(async move { Engine::new(plan(vec![group])).run(sender).await });
    assert!(matches!(
        receiver.recv().await,
        Some(RunEvent::Started { .. })
    ));
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(1)).await;
    task.await.unwrap().unwrap();
    assert!(matches!(
        receiver.recv().await,
        Some(RunEvent::Finished { .. })
    ));
    assert!(receiver.recv().await.is_none());
}

#[tokio::test]
async fn receiver_closure_interrupts_condition_only_infinite_while() {
    let group = group(vec![Node::While(WhileController::new(
        "forever",
        true,
        vec![Node::If(IfController::new(
            "skip",
            false,
            vec![Node::Sample(TestSampler::Increment)],
        ))],
    ))]);
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let task = tokio::spawn(async move { Engine::new(plan(vec![group])).run(sender).await });
    assert!(matches!(
        receiver.recv().await,
        Some(RunEvent::Started { .. })
    ));
    tokio::task::yield_now().await;
    drop(receiver);
    let result = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(result, Err(EngineError::EventsClosed)));
}
#[test]
fn functions_in_conditions_write_variables_and_short_circuit() {
    let mut context = UserContext::new("functions", 0);
    assert!(
        Condition::from("${__changeCase(TRUE,lower,flag)}")
            .evaluate(&mut context)
            .unwrap()
    );
    assert_eq!(context.variables["flag"], "true");
    let condition = Condition::Any(vec![
        Condition::Constant(true),
        Condition::from("${__Random(1,1,unreached)}"),
    ]);
    assert!(condition.evaluate(&mut context).unwrap());
    assert!(!context.variables.contains_key("unreached"));
    assert!(
        Condition::from("${__changeCase(TRUE,lower,rolledBack)}${missing}")
            .evaluate(&mut context)
            .is_err()
    );
    assert!(!context.variables.contains_key("rolledBack"));
    context.variables.insert("iteration".into(), "999".into());
    assert!(
        Condition::from("${__changeCase(${iteration},lower,captured)}")
            .evaluate(&mut context)
            .is_ok()
    );
    assert_eq!(context.variables["captured"], "0");
    assert_eq!(context.variables["iteration"], "999");
}
