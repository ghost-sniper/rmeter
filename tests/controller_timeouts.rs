use rmeter::{
    RunEvent,
    core::{plan::Node, sampler::SampleResult},
    parse_script, run_script,
};
use serde_json::{Value, json};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc,
};

fn plan(children: Vec<Value>) -> Value {
    json!({"version":1,"name":"timeouts","groups":[{"name":"users","on_error":"continue","children":children}]})
}
fn js(name: &str, script: &str) -> Value {
    json!({"type":"javascript","name":name,"script":script,"timeout_ms":10000})
}
async fn run(source: Value) -> Vec<SampleResult> {
    tokio::time::timeout(Duration::from_secs(15), async {
        let (tx, mut rx) = mpsc::channel(16);
        let (result, samples) =
            tokio::join!(async { run_script(&source.to_string(), tx).await }, async {
                let mut samples = Vec::new();
                while let Some(event) = rx.recv().await {
                    if let RunEvent::SampleCompleted(event) = event {
                        samples.push(event.result);
                    }
                }
                samples
            });
        result.unwrap();
        samples
    })
    .await
    .expect("controller cancellation must finish promptly")
}
#[test]
fn timeout_schema_defaults_names_and_invalid_values() {
    for kind in ["loop", "while"] {
        let mut node = json!({"type":kind,"name":"named","children":[]});
        if kind == "loop" {
            node["count"] = json!(1);
        } else {
            node["condition"] = json!(true);
        }
        for value in [None, Some(json!(0)), Some(json!(1234))] {
            if let Some(value) = value {
                node["timeout_ms"] = value;
            } else {
                node.as_object_mut().unwrap().remove("timeout_ms");
            }
            let parsed = parse_script(&plan(vec![node.clone()]).to_string()).unwrap();
            match &parsed.groups[0].children[0] {
                Node::LoopController(c) => {
                    assert_eq!(c.generic.name, "named");
                    assert_eq!(
                        c.timeout.as_millis(),
                        node["timeout_ms"].as_u64().unwrap_or(0) as u128
                    );
                }
                Node::While(c) => assert_eq!(
                    c.timeout.as_millis(),
                    node["timeout_ms"].as_u64().unwrap_or(0) as u128
                ),
                _ => panic!("expected named controller"),
            }
        }
        for invalid in [
            json!(null),
            json!(-1),
            json!(1.5),
            json!("${timeout}"),
            json!(9007199254740992u64),
        ] {
            node["timeout_ms"] = invalid;
            assert!(parse_script(&plan(vec![node.clone()]).to_string()).is_err());
        }
    }
    let parsed =
        parse_script(&plan(vec![json!({"type":"loop","count":1,"children":[]})]).to_string())
            .unwrap();
    let Node::LoopController(c) = &parsed.groups[0].children[0] else {
        panic!()
    };
    assert_eq!(c.generic.name, "Loop 循环");
}
#[tokio::test]
async fn real_worker_sampler_processors_and_timer_cancel_without_duplicate_results() {
    for stage in ["sampler", "pre_processors", "post_processors", "timers"] {
        let mut victim = js("interrupted", "vars.put('sample_finished','yes');");
        match stage {
            "sampler" => victim["script"] = json!("while(true){}"),
            "timers" => {
                victim[stage] = json!([{"type":"constant_timer","name":"wait","delay_ms":60000}])
            }
            _ => victim[stage] = json!([js("endless", "while(true){}")]),
        }
        let mut first = js("completed", "vars.put('completed','yes');");
        first["post_processors"] = json!([js("commit", "vars.put('post_done','yes');")]);
        let samples = run(plan(vec![js("warmup", ""), json!({"type":"loop","name":"budget","count":5,"timeout_ms":300,"children":[first,victim,js("skipped", "throw Error('must skip');")]}), js("after", "if(vars.get('completed')!=='yes'||vars.get('post_done')!=='yes') throw Error('state');")])).await;
        assert_eq!(
            samples.iter().map(|s| s.label.as_str()).collect::<Vec<_>>(),
            ["warmup", "completed", "budget", "after"],
            "{stage}: {samples:?}"
        );
        assert_eq!(
            samples.iter().filter(|s| !s.success).count(),
            1,
            "{stage}: {samples:?}"
        );
        assert_eq!(
            samples[2].response_code.as_deref(),
            Some("ControllerTimeout")
        );
        assert!(samples[2].elapsed >= Duration::from_millis(300));
    }
}
#[tokio::test]
async fn http_wait_is_cancelled_and_later_request_still_runs() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut blocked, _) = listener.accept().await.unwrap();
        let mut buffer = [0; 4096];
        assert!(blocked.read(&mut buffer).await.unwrap() > 0);
        // Dropping the request future must close the outstanding HTTP connection.
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(3), blocked.read(&mut buffer))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        let (mut next, _) = listener.accept().await.unwrap();
        assert!(next.read(&mut buffer).await.unwrap() > 0);
        next.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
            .await
            .unwrap();
    });
    let request = json!({"type":"http_request","name":"HTTP","url":url});
    let samples = run(plan(vec![json!({"type":"while","name":"HTTP budget","condition":true,"timeout_ms":150,"children":[request.clone()]}),request])).await;
    assert_eq!(samples.len(), 2);
    assert_eq!(
        samples[0].response_code.as_deref(),
        Some("ControllerTimeout")
    );
    assert!(samples[1].success);
    server.await.unwrap();
}
#[tokio::test]
#[ignore = "requires isolated PostgreSQL through RMETER_POSTGRES_URL"]
async fn postgres_timeout_cancels_query_and_releases_single_connection_pool() {
    let url = std::env::var("RMETER_POSTGRES_URL").expect("RMETER_POSTGRES_URL");
    let query = |name: &str, sql: &str| json!({"type":"jdbc_request","name":name,"connection":"db","sql":sql});
    let mut source = plan(vec![
        query("warmup", "SELECT 1"),
        json!({"type":"loop","name":"SQL budget","count":3,"timeout_ms":150,"children":[query("interrupted", "SELECT pg_sleep(60)")]}),
        query("after", "SELECT 1"),
    ]);
    source["jdbc_connections"] =
        json!([{"name":"DB","variable_name":"db","url":url,"max_connections":1}]);
    let samples = run(source).await;
    assert_eq!(samples.len(), 3, "{samples:?}");
    assert!(samples[0].success, "{samples:?}");
    assert_eq!(
        samples[1].response_code.as_deref(),
        Some("ControllerTimeout")
    );
    assert!(samples[2].success, "{samples:?}");
    // Observe server-side cancellation, not only a returned client-side timeout.
    use rmeter::component::{JdbcConnection, JdbcConnectionConfig, JdbcSampler, JdbcSamplerConfig};
    use rmeter::core::{context::UserContext, sampler::Sampler};
    let connection = std::sync::Arc::new(
        JdbcConnection::new(JdbcConnectionConfig::new(
            "observer",
            std::env::var("RMETER_POSTGRES_URL").unwrap(),
        ))
        .unwrap(),
    );
    let observer = JdbcSampler::new(JdbcSamplerConfig::new("observe", "observer", "SELECT count(*) FROM pg_stat_activity WHERE application_name='rmeter' AND state='active' AND query='SELECT pg_sleep(60)'"), connection).unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let result = observer
                .sample(&mut UserContext::new("observer", 0))
                .await
                .unwrap();
            assert!(result.success);
            let body: Value =
                serde_json::from_slice(result.response_body.as_ref().unwrap()).unwrap();
            if body["rows"][0][0] == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("cancelled SQL must disappear on the server");
}
