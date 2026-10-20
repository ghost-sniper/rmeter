mod support;

use std::{future::Future, time::Duration};

use rmeter::{
    ScriptError,
    component::Setting,
    core::{
        context::UserContext,
        controller::{CursorStep, ExecutionCursor},
        plan::{ErrorPolicy, LoopCount},
    },
    load_script, parse_script,
    report::{ReportEvent, RunSummary},
    run_script, run_script_file,
};
use serde_json::{Value, json};
use support::{TempScript, script, server};
use tokio::sync::mpsc;

#[test]
fn jdbc_connections_compile_with_http_and_are_shared_across_groups() {
    let source = json!({"version":1,"name":"mixed", "jdbc_connections":[{
        "name":"database","variable_name":"db","driver":"postgresql","url":"postgresql://localhost/test"
    }],"groups":[
        {"name":"a","children":[{"type":"http_request","name":"HTTP","url":"http://localhost/"},
            {"type":"jdbc_request","name":"SQL","connection":"db","sql":"SELECT $1::integer","parameters":[1]}]},
        {"name":"b","children":[{"type":"jdbc_request","name":"SQL","connection":"db","sql":"SELECT 1"}]}
    ]});
    let plan = parse_script(&source.to_string()).unwrap();
    let rmeter::core::plan::Node::Sample(http) = &plan.groups[0].children[0] else {
        panic!()
    };
    assert!(http.as_http().is_some());
    let rmeter::core::plan::Node::Sample(first) = &plan.groups[0].children[1] else {
        panic!()
    };
    let rmeter::core::plan::Node::Sample(second) = &plan.groups[1].children[0] else {
        panic!()
    };
    assert!(std::sync::Arc::ptr_eq(
        first.as_jdbc().unwrap().connection(),
        second.as_jdbc().unwrap().connection()
    ));
}

#[test]
fn jdbc_scripts_reject_missing_connections_and_invalid_configuration() {
    let source = json!({"version":1,"name":"jdbc","groups":[{"name":"g","children":[
        {"type":"jdbc_request","name":"query","connection":"db","sql":"SELECT 1"},
        {"type":"jdbc_connection_configuration","name":"connection","variable_name":"db","url":"postgresql://localhost/test"}
    ]}]});
    assert_eq!(
        parse_script(&source.to_string()).unwrap().groups[0]
            .children
            .len(),
        1
    );
    let mut missing = source.clone();
    missing["groups"][0]["children"][0]["connection"] = json!("missing");
    assert!(
        parse_script(&missing.to_string())
            .unwrap_err()
            .to_string()
            .contains("was not configured")
    );
    for (field, invalid) in [
        ("driver", json!("mysql")),
        ("max_connections", json!(0)),
        ("connect_timeout_ms", json!(0)),
        ("url", json!("jdbc:mysql://localhost/test")),
        ("unknown_setting", json!(true)),
    ] {
        let mut invalid_source = source.clone();
        invalid_source["groups"][0]["children"][1][field] = invalid;
        assert!(
            parse_script(&invalid_source.to_string()).is_err(),
            "{field}"
        );
    }
    let mut inline_url = source;
    inline_url["groups"][0]["children"][0]["url"] = json!("postgresql://localhost/other");
    assert!(parse_script(&inline_url.to_string()).is_err());
}

/// A broken run or a missing request must fail the test instead of hanging it.
async fn within<T>(operation: &str, future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .unwrap_or_else(|_| panic!("timed out waiting for {operation}"))
}

#[test]
fn parses_group_defaults_and_rejects_bad_schema() {
    let source = script("http://localhost/status");
    let plan = parse_script(&source.to_string()).unwrap();
    let group = &plan.groups[0];
    assert_eq!(group.users, 1);
    assert_eq!(group.iterations, LoopCount::Finite(1));
    assert_eq!(group.on_error, ErrorPolicy::Continue);
    assert_eq!(group.ramp_up, Duration::ZERO);
    assert_eq!(group.max_duration, None);
    let invalid: Vec<Value> = vec![
        {
            let mut v = source.clone();
            v["version"] = json!(2);
            v
        },
        {
            let mut v = source.clone();
            v["name"] = json!(" ");
            v
        },
        {
            let mut v = source.clone();
            v["groups"][0]["user"] = json!(2);
            v
        },
        {
            let mut v = source.clone();
            v["groups"][0]["iterations"] = json!(-1);
            v
        },
        {
            let mut v = source.clone();
            v["groups"][0]["iterations"] = json!("forever");
            v
        },
        {
            let mut v = source.clone();
            v["groups"][0]["on_error"] = json!("ignore");
            v
        },
        {
            let mut v = source.clone();
            v["variables"] = json!({"n": 42});
            v
        },
        {
            let mut v = source.clone();
            v["groups"][0]["children"][0]["settings"] = json!({"request_timout_ms": 5});
            v
        },
        {
            let mut v = source.clone();
            v["groups"][0]["children"][0]["method"] = json!("GET\n");
            v
        },
        {
            let mut v = source.clone();
            v["groups"][0]["children"][0]["assertions"][0]["path"] = json!("$[");
            v
        },
        {
            let mut v = source.clone();
            v["groups"][0]["children"][0]["assertions"][0]["matcher"] =
                json!({"type": "regex", "pattern": "["});
            v
        },
    ];
    for value in invalid {
        assert!(
            parse_script(&value.to_string()).is_err(),
            "accepted {value}"
        );
    }
    let error = parse_script("{\n invalid").unwrap_err().to_string();
    assert!(error.contains("line 2"), "{error}");
}

#[test]
fn preserves_timeout_inheritance_disable_and_scoped_defaults() {
    let source = json!({"version":1, "name":"defaults", "defaults": {
        "host":"localhost", "request_timeout_ms":100, "connect_timeout_ms":50,
        "headers":[["Accept", "application/json"]]
    }, "groups":[{"name":"g", "children":[
        {"type":"http_request", "name":"inherit"},
        {"type":"sequence", "children":[
            {"type":"http_request", "name":"disable", "settings":{"request_timeout_ms":null}},
            {"type":"http_defaults", "settings":{"connect_timeout_ms":null, "headers":[]}}
        ]},
        {"type":"http_request", "name":"outside"}
    ]}]});
    let plan = parse_script(&source.to_string()).unwrap();
    let mut context = UserContext::new("g", 0);
    let mut cursor = ExecutionCursor::new(&plan.groups[0].children);
    let mut settings = Vec::new();
    for _ in 0..3 {
        let CursorStep::Sample(sampler) = cursor.step(&mut context).unwrap() else {
            panic!("expected one of the three configured requests");
        };
        settings.push(sampler.as_http().unwrap().config().settings.clone());
    }
    assert!(matches!(
        cursor.step(&mut context).unwrap(),
        CursorStep::Done
    ));
    assert_eq!(settings.len(), 3);
    assert_eq!(
        settings[0].request_timeout,
        Setting::Value(Some(Duration::from_millis(100)))
    );
    assert_eq!(settings[1].request_timeout, Setting::Value(None));
    assert_eq!(settings[1].connect_timeout, Setting::Value(None));
    assert_eq!(settings[1].headers, Some(vec![]));
    assert_eq!(settings[2].connect_timeout, settings[0].connect_timeout);
    assert_eq!(settings[2].headers, settings[0].headers);
}

#[test]
fn compiles_controls_and_structured_conditions_with_runtime_state() {
    let request = json!({"type":"http_request", "name":"work", "url":"http://localhost/"});
    let source = json!({"version":1,"name":"controls","groups":[{
        "name":"g", "iterations":"infinite", "max_duration_ms":25, "on_error":"next_iteration",
        "children":[{"type":"controller","name":"generic","children":[
            {"type":"loop","count":2,"children":[{"type":"if", "name":"first user",
                "condition":{"type":"all","conditions":[true,{"type":"not","condition":false},
                    {"type":"compare","left":{"type":"user_index"},"operator":"equal",
                        "right":{"type":"literal","value":"0"}}]},"children":[request.clone()]}]},
            {"type":"while","name":"until failure", "condition":"LAST", "children":[request]},
            {"type":"if","name":"skip","condition":false,"children":[]}
        ]}]
    }]});
    let plan = parse_script(&source.to_string()).unwrap();
    assert_eq!(plan.groups[0].iterations, LoopCount::Infinite);
    assert_eq!(plan.groups[0].max_duration, Some(Duration::from_millis(25)));
    assert_eq!(plan.groups[0].on_error, ErrorPolicy::NextIteration);
    let mut context = UserContext::new("g", 0);
    let mut cursor = ExecutionCursor::new(&plan.groups[0].children);
    for _ in 0..3 {
        assert!(matches!(
            cursor.step(&mut context).unwrap(),
            CursorStep::Sample(_)
        ));
    }
    context.last_sample_ok = false;
    assert!(matches!(
        cursor.step(&mut context).unwrap(),
        CursorStep::Done
    ));
}

#[tokio::test]
async fn parsed_bodies_reach_server_and_all_matcher_types_execute() {
    let bodies = [
        (json!({"type":"empty"}), ""),
        (json!({"type":"text","value":"hello"}), "hello"),
        (json!({"type":"bytes","value":[0,127]}), "\0\u{7f}"),
        (
            json!({"type":"form","value":[["a","1"],["a","2"]]}),
            "a=1&a=2",
        ),
        (
            json!({"type":"json","value":{"user":"${user}"}}),
            r#"{"user":"Alice"}"#,
        ),
    ];
    for (body, expected) in bodies {
        let (url, task) = server(r#"{"a":null,"items":["ok","ok"]}"#, 1).await;
        let mut source = script(&url);
        source["variables"] = json!({"user":"Alice"});
        source["groups"][0]["children"][0]["method"] = json!("POST");
        source["groups"][0]["children"][0]["body"] = body;
        source["groups"][0]["children"][0]["assertions"] = json!([
            {"name":"a","path":"$.a","matcher":{"type":"equals","expected":null}},
            {"name":"b","path":"$.items[*]","matcher":{"type":"regex","pattern":"^ok$"},"selection":"all"},
            {"name":"c","path":"$.a","matcher":{"type":"exists"},"selection":"any"},
            {"name":"d","path":"$.b","matcher":{"type":"not_exists"}}
        ]);
        let (sender, mut receiver) = mpsc::channel(16);
        let source = source.to_string();
        let (result, summary) = within("request body run", async {
            tokio::join!(run_script(&source, sender), async {
                let mut summary = RunSummary::default();
                while let Some(event) = receiver.recv().await {
                    summary.observe(&event);
                    if let rmeter::RunEvent::SampleCompleted(sample) = event {
                        assert!(sample.result.success, "{:?}", sample.result.assertions);
                        assert_eq!(sample.result.assertions.len(), 4);
                        assert!(
                            sample
                                .result
                                .assertions
                                .iter()
                                .all(|assertion| assertion.success)
                        );
                    }
                }
                summary
            })
        })
        .await;
        result.unwrap();
        assert_eq!(summary.samples, 1);
        assert_eq!(summary.passed, 1);
        assert!(summary.finished);
        let requests = within("request body server", task).await.unwrap();
        assert_eq!(requests[0].split_once("\r\n\r\n").unwrap().1, expected);
    }
}

#[tokio::test]
async fn library_executes_script_and_produces_serializable_assertion_events() {
    let (url, task) = server(r#"{"code":7}"#, 1).await;
    let (sender, mut receiver) = mpsc::channel(1);
    let mut summary = RunSummary::default();
    let mut reports = Vec::new();
    let source = script(&url).to_string();
    let (result, ()) = within("assertion event run", async {
        tokio::join!(run_script(&source, sender), async {
            while let Some(event) = receiver.recv().await {
                summary.observe(&event);
                let encoded = serde_json::to_string(&ReportEvent::from(event)).unwrap();
                let _: ReportEvent = serde_json::from_str(&encoded).unwrap();
                reports.push(serde_json::from_str::<Value>(&encoded).unwrap());
            }
        })
    })
    .await;
    result.unwrap();
    within("assertion event server", task).await.unwrap();
    assert_eq!(summary.samples, 1);
    assert_eq!(summary.failed, 1);
    assert!(summary.finished);
    assert_eq!(reports.len(), 3);
    assert_eq!(reports[0]["type"], "started");
    assert_eq!(reports[1]["response_code"], "200");
    assert_eq!(reports[1]["success"], false);
    assert_eq!(reports[1]["assertions"][0]["name"], "code");
    assert!(
        reports[1]["assertions"][0]["message"]
            .as_str()
            .unwrap()
            .contains("$.code")
    );
    assert_eq!(reports[2]["type"], "finished");
}

#[tokio::test]
async fn loaded_file_expands_variables_and_json_body_then_runs() {
    let (url, task) = server(r#"{"code":0}"#, 1).await;
    let mut source = script("${url}");
    source["variables"] = json!({"url":url, "user":"quoted \"name\""});
    source["groups"][0]["children"][0]["method"] = json!("POST");
    source["groups"][0]["children"][0]["body"] = json!({"type":"json","value":{"user":"${user}"}});
    let file = TempScript::new(&format!("\u{feff}{source}"));
    let (sender, mut receiver) = mpsc::channel(16);
    let (result, summary) = within("script file run", async {
        tokio::join!(run_script_file(&file.0, sender), async {
            let mut summary = RunSummary::default();
            while let Some(event) = receiver.recv().await {
                summary.observe(&event);
            }
            summary
        })
    })
    .await;
    result.unwrap();
    assert_eq!(summary.samples, 1);
    assert_eq!(summary.passed, 1);
    assert!(summary.finished);
    let requests = within("script file server", task).await.unwrap();
    assert!(requests[0].starts_with("POST /status HTTP/1.1"));
    let body: Value = serde_json::from_str(requests[0].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(body["user"], "quoted \"name\"");
}

#[tokio::test]
async fn reports_file_errors_and_event_receiver_cancellation() {
    let file = TempScript::new("invalid JSON");
    assert!(matches!(
        load_script(&file.0).await,
        Err(ScriptError::Json(_))
    ));
    let missing = file.0.with_extension("does-not-exist");
    let error = load_script(&missing).await.unwrap_err();
    assert!(matches!(error, ScriptError::Io { .. }));
    assert!(
        error
            .to_string()
            .contains(missing.file_name().unwrap().to_str().unwrap())
    );
    let (sender, receiver) = mpsc::channel(1);
    drop(receiver);
    let source = script("http://localhost/").to_string();
    let result = within("closed event receiver", run_script(&source, sender)).await;
    assert!(matches!(result, Err(ScriptError::Engine(_))));
}

#[tokio::test]
async fn aborting_library_run_cancels_in_flight_http_and_closes_events() {
    use tokio::{io::AsyncReadExt, net::TcpListener};
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let source = script(&format!("http://{}/", listener.local_addr().unwrap())).to_string();
    let (received, ready) = tokio::sync::oneshot::channel();
    let pending_server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = stream.read(&mut buffer).await.unwrap();
            assert!(count > 0);
            request.extend_from_slice(&buffer[..count]);
        }
        received.send(()).unwrap();
        // Deliberately do not send a response: the sampler must be cancelled.
        assert_eq!(stream.read(&mut buffer).await.unwrap(), 0);
    });
    let (sender, mut receiver) = mpsc::channel(16);
    let runner = tokio::spawn(async move { run_script(&source, sender).await });
    within("pending HTTP request", ready).await.unwrap();
    runner.abort();
    assert!(
        within("aborted library run", runner)
            .await
            .unwrap_err()
            .is_cancelled()
    );
    within("cancelled HTTP and event channel", async {
        while let Some(event) = receiver.recv().await {
            assert!(matches!(event, rmeter::RunEvent::Started { .. }));
        }
        pending_server.await.unwrap();
    })
    .await;
}

#[test]
fn shipped_example_compiles_without_network_requests() {
    parse_script(include_str!("../examples/http.json")).unwrap();
    parse_script(include_str!("../examples/assertions.json")).unwrap();
    parse_script(include_str!("../examples/jdbc_postgresql.json")).unwrap();
}

#[tokio::test]
async fn regex_and_xml_failures_update_controller_state_before_the_next_request() {
    for (body, assertion) in [
        (
            "ready",
            json!({"type":"regex","name":"must fail","pattern":"ERROR"}),
        ),
        (
            "<root><code>7</code></root>",
            json!({"type":"xml","name":"must fail","xpath":"/root/code","matcher":{"type":"equals","expected":"0"}}),
        ),
    ] {
        let (url, task) = server(body, 4).await;
        let source = json!({"version":1,"name":"assertion state","groups":[{
            "name":"g","iterations":2,"on_error":"continue","children":[
                {"type":"if","name":"check every sample","condition":{"type":"last_sample_ok"},"evaluate_all":true,"children":[
                    {"type":"http_request","name":"asserted","url":url,"assertions":[assertion]},
                    {"type":"http_request","name":"skip IF","url":url}
                ]},
                {"type":"while","name":"skip While","condition":"LAST","children":[{"type":"http_request","name":"skip While","url":url}]},
                {"type":"http_request","name":"after","url":url}
            ]
        }]}).to_string();
        let (sender, mut receiver) = mpsc::channel(1);
        let (result, samples) = within("assertion controller state", async {
            tokio::join!(run_script(&source, sender), async {
                let mut samples = Vec::new();
                while let Some(event) = receiver.recv().await {
                    if let rmeter::RunEvent::SampleCompleted(sample) = event {
                        samples.push((
                            sample.result.label,
                            sample.iteration,
                            sample.result.success,
                        ));
                    }
                }
                samples
            })
        })
        .await;
        result.unwrap();
        within("assertion state server", task).await.unwrap();
        assert_eq!(
            samples,
            vec![
                ("asserted".into(), 0, false),
                ("after".into(), 0, true),
                ("asserted".into(), 1, false),
                ("after".into(), 1, true)
            ]
        );
    }
}

#[tokio::test]
async fn typed_assertion_components_execute_and_legacy_json_stays_compatible() {
    let cases = [
        (
            r#"{"code":0}"#,
            json!([
                {"type":"json","name":"typed JSON","path":"$.code","matcher":{"type":"equals","expected":0}},
                {"type":"regex","name":"body regex","pattern":"code"},
                {"name":"legacy JSON","path":"$.code","matcher":{"type":"equals","expected":0}}
            ]),
            vec![true, true, true],
        ),
        (
            r#"<r xmlns="urn:test"><code>0</code></r>"#,
            json!([
                {"type":"xml","name":"XML","xpath":"/n:r/n:code/text()","namespaces":{"n":"urn:test"},
                    "matcher":{"type":"equals","expected":"0"}},
                {"type":"regex","name":"body regex","pattern":"<code>0</code>"},
                {"type":"json","name":"invalid JSON","path":"$.missing","matcher":{"type":"not_exists"}}
            ]),
            vec![true, true, false],
        ),
        (
            "ready\nstatus=ok",
            json!([
                {"type":"regex","name":"multiline","pattern":"(?m)^status=ok$"},
                {"type":"regex","name":"no error","pattern":"ERROR","invert":true}
            ]),
            vec![true, true],
        ),
    ];
    for (body, assertions, expected) in cases {
        let (url, task) = server(body, 1).await;
        let mut source = script(&url);
        source["groups"][0]["children"][0]["assertions"] = assertions;
        let source = source.to_string();
        let (sender, mut receiver) = mpsc::channel(1);
        let (result, samples) = within("typed assertions", async {
            tokio::join!(run_script(&source, sender), async {
                let mut samples = Vec::new();
                while let Some(event) = receiver.recv().await {
                    if let rmeter::RunEvent::SampleCompleted(sample) = event {
                        samples.push(sample);
                    }
                }
                samples
            })
        })
        .await;
        result.unwrap();
        within("typed assertion server", task).await.unwrap();
        assert_eq!(samples.len(), 1);
        assert_eq!(
            samples[0].result.success,
            expected.iter().all(|success| *success)
        );
        assert_eq!(
            samples[0]
                .result
                .assertions
                .iter()
                .map(|a| a.success)
                .collect::<Vec<_>>(),
            expected
        );
    }
}

#[test]
fn malformed_or_mixed_component_fields_are_rejected() {
    for assertion in [
        json!({"type":"unknown","name":"unknown","path":"$","matcher":{"type":"exists"}}),
        json!({"type":"regex","name":"invalid","pattern":"["}),
        json!({"type":"regex","name":"wrong field","pattern":"ok","path":"$"}),
        json!({"type":"xml","name":"invalid","xpath":"/root[","matcher":{"type":"exists"}}),
        json!({"type":"xml","name":"unbound prefix","xpath":"/x:root","matcher":{"type":"exists"}}),
        json!({"type":"xml","name":"wrong type","xpath":"/root","matcher":{"type":"equals","expected":0}}),
        json!({"type":"json","name":"wrong field","path":"$","pattern":".*","matcher":{"type":"exists"}}),
    ] {
        let mut source = script("http://localhost/");
        source["groups"][0]["children"][0]["assertions"] = json!([assertion]);
        assert!(
            parse_script(&source.to_string()).is_err(),
            "accepted {source}"
        );
    }
}
