use rmeter::component::ComponentSampler;
use rmeter::core::plan::Node;
use rmeter::{parse_script, report::ReportEvent, run_script};
use serde_json::{Value, json};
use std::time::{Duration, Instant};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc,
};

fn constant(ms: Value) -> Value {
    json!({"type":"constant_timer","name":"pause","delay_ms":ms})
}
fn gaussian(deviation: Value, offset: Value) -> Value {
    json!({"type":"gaussian_random_timer","name":"random","deviation_ms":deviation,"offset_ms":offset})
}
fn http(url: &str) -> Value {
    json!({"type":"http_request","name":"HTTP","url":url})
}
fn plan(children: Vec<Value>) -> Value {
    json!({"version":1,"name":"timers","groups":[{"name":"users","children":children}]})
}
fn timer_count(node: &Node<ComponentSampler>) -> usize {
    let Node::Sample(sampler) = node else {
        panic!("expected sampler")
    };
    if let Some(http) = sampler.as_http() {
        http.config().timers.len()
    } else if let Some(jdbc) = sampler.as_jdbc() {
        jdbc.config().timers.len()
    } else {
        sampler.as_javascript().unwrap().config().timers.len()
    }
}

#[test]
fn timer_scopes_accumulate_independent_of_tree_order_without_leaking() {
    let mut local = http("http://localhost");
    local["timers"] = json!([constant(json!(3)), gaussian(json!(0), json!(4))]);
    let mut source = plan(vec![
        http("http://localhost"),
        json!({"type":"loop","count":2,"children":[local, constant(json!(2))]}),
        http("http://localhost"),
        constant(json!(1)),
        json!({"type":"constant_timer","enabled":false,"delay_ms":"invalid"}),
    ]);
    source["timers"] = json!([constant(json!(5))]);
    source["groups"]
        .as_array_mut()
        .unwrap()
        .push(json!({"name":"other","children":[http("http://localhost")]}));
    let compiled = parse_script(&source.to_string()).unwrap();
    let children = &compiled.groups[0].children;
    assert_eq!(children.len(), 3, "timers do not become execution nodes");
    assert_eq!(timer_count(&children[0]), 2);
    let Node::LoopController(controller) = &children[1] else {
        panic!()
    };
    assert_eq!(timer_count(&controller.generic.children[0]), 5);
    assert_eq!(timer_count(&children[2]), 2);
    assert_eq!(timer_count(&compiled.groups[1].children[0]), 1);
}

#[test]
fn timer_validation_and_disabled_drafts() {
    for bad in [
        json!(-1),
        json!(0.5),
        json!(null),
        json!(true),
        json!("NaN"),
        json!("${broken"),
        json!(9007199254740992u64),
    ] {
        assert!(
            parse_script(&plan(vec![constant(bad.clone())]).to_string()).is_err(),
            "{bad}"
        );
    }
    for good in [json!(0), json!("0"), json!("${delay}")] {
        assert!(parse_script(&plan(vec![constant(good)]).to_string()).is_ok());
    }
    assert!(parse_script(&plan(vec![gaussian(json!(0.5), json!(0))]).to_string()).is_ok());
    for timer in [
        gaussian(json!(-1), json!(0)),
        gaussian(json!(0), json!(0.5)),
    ] {
        assert!(parse_script(&plan(vec![timer]).to_string()).is_err());
    }
    let mut source = plan(vec![
        json!({"type":"constant_timer","enabled":false,"delay_ms":null}),
    ]);
    source["timers"] = json!([{"enabled":false,"type":"gaussian_random_timer","deviation_ms":-1}]);
    assert!(parse_script(&source.to_string()).is_ok());
    source["enabled"] = json!(false);
    source["timers"] = json!("invalid draft");
    assert!(parse_script(&source.to_string()).is_ok());
    let mut misplaced = http("http://localhost");
    misplaced["post_processors"] = json!([constant(json!(0))]);
    assert!(parse_script(&plan(vec![misplaced]).to_string()).is_err());
}

async fn server(requests: usize) -> (String, tokio::task::JoinHandle<Vec<Instant>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let mut arrivals = vec![];
        for _ in 0..requests {
            let (mut socket, _) = listener.accept().await.unwrap();
            arrivals.push(Instant::now());
            let mut bytes = vec![];
            let mut buffer = [0; 4096];
            while !bytes.windows(4).any(|b| b == b"\r\n\r\n") {
                let count = socket.read(&mut buffer).await.unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&buffer[..count]);
            }
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .await
                .unwrap();
        }
        arrivals
    });
    (url, task)
}
async fn run(source: Value) -> (rmeter::report::RunSummary, Vec<ReportEvent>) {
    let (tx, mut rx) = mpsc::channel(1);
    let task = tokio::spawn(async move { run_script(&source.to_string(), tx).await.unwrap() });
    let mut events = vec![];
    let mut summary = rmeter::report::RunSummary::default();
    while let Some(event) = rx.recv().await {
        summary.observe(&event);
        events.push(event.into());
    }
    task.await.unwrap();
    (summary, events)
}

#[tokio::test]
async fn timers_apply_per_loop_and_iteration_not_to_false_branches_or_empty_scopes() {
    let (url, server) = server(4).await;
    let mut sample = http(&url);
    sample["timers"] = json!([gaussian(json!(0), json!(20))]);
    let mut source = plan(vec![
        json!({"type":"if","name":"skip","condition":false,"children":[constant(json!(60000)),http(&url)]}),
        json!({"type":"loop","count":2,"children":[sample,constant(json!(30))]}),
        json!({"type":"loop","count":1,"children":[constant(json!(60000))]}),
        constant(json!(10)),
    ]);
    source["groups"][0]["iterations"] = json!(2);
    source["timers"] = json!([constant(json!(10))]);
    let started = Instant::now();
    let (summary, events) = tokio::time::timeout(Duration::from_secs(10), run(source))
        .await
        .unwrap();
    let arrivals = server.await.unwrap();
    assert_eq!(summary.samples, 4);
    assert_eq!(summary.passed, 4);
    let mut previous = started;
    for arrival in arrivals {
        assert!(arrival.duration_since(previous) >= Duration::from_millis(65));
        previous = arrival;
    }
    let response_time: f64 = events
        .iter()
        .filter_map(|e| {
            if let ReportEvent::SampleCompleted { elapsed_ms, .. } = e {
                Some(*elapsed_ms)
            } else {
                None
            }
        })
        .sum();
    assert!(
        started.elapsed().as_secs_f64() * 1000.0 - response_time >= 260.0,
        "timers must not count toward response time"
    );
}

#[tokio::test]
async fn timer_variables_resolve_after_preprocessors_and_before_http_and_javascript() {
    let (url, server) = server(1).await;
    let pre = json!({"type":"javascript","name":"set delay","script":"vars.put('delay','80');","timeout_ms":2000});
    for mut sample in [
        http(&url),
        json!({"type":"javascript","name":"JS","script":"if(vars.get('delay')!=='80') throw Error('vars');","timeout_ms":2000}),
    ] {
        sample["pre_processors"] = json!([pre]);
        sample["timers"] = json!([constant(json!("${delay}"))]);
        let started = Instant::now();
        let (summary, _) = run(plan(vec![sample])).await;
        assert_eq!(summary.passed, 1);
        assert!(started.elapsed() >= Duration::from_millis(75));
    }
    server.await.unwrap();
    // JDBC timer failure occurs before obtaining a database connection.
    let mut source = plan(vec![
        json!({"type":"jdbc_request","name":"SQL","connection":"db","sql":"SELECT 1","timers":[constant(json!("${missing}"))]}),
    ]);
    source["jdbc_connections"] = json!([{"name":"DB","variable_name":"db","url":"postgresql://127.0.0.1:1/test?sslmode=disable"}]);
    let (summary, events) = run(source).await;
    assert_eq!(summary.samples, 1);
    assert_eq!(summary.failed, 1);
    assert!(events.iter().any(|e| matches!(e,ReportEvent::SampleCompleted{response_code:Some(code),..} if code=="TimerError")));
}

#[tokio::test]
async fn deadline_and_dropping_engine_cancel_wait_without_requests_or_samples() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let mut source = plan(vec![http(&url), constant(json!(60000))]);
    source["groups"][0]["max_duration_ms"] = json!(35);
    let (summary, _) = tokio::time::timeout(Duration::from_secs(2), run(source.clone()))
        .await
        .unwrap();
    assert_eq!(summary.samples, 0);
    source["groups"][0]
        .as_object_mut()
        .unwrap()
        .remove("max_duration_ms");
    let (tx, mut rx) = mpsc::channel(16);
    let engine = rmeter::core::engine::Engine::new(parse_script(&source.to_string()).unwrap());
    assert!(
        tokio::time::timeout(Duration::from_millis(40), engine.run(tx))
            .await
            .is_err()
    );
    while let Some(event) = rx.recv().await {
        assert!(!matches!(
            event,
            rmeter::core::event::RunEvent::SampleCompleted(_)
        ));
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(40), listener.accept())
            .await
            .is_err()
    );
}
