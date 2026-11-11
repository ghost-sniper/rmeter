use std::{collections::HashMap, time::Duration};

use rmeter_component::{
    AssertionConfig, HttpBody, HttpMethod, HttpNode, HttpRequestConfig, HttpRequestDefaults,
    HttpSampler, JsonAssertionConfig, RegexAssertionConfig, Setting, XmlAssertionConfig,
    compile_http_nodes,
};
use rmeter_core::{
    context::UserContext,
    engine::Engine,
    event::RunEvent,
    plan::{ErrorPolicy, LoopCount, TestPlan, ThreadGroup},
    sampler::Sampler,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    task::JoinHandle,
};

struct Response {
    status: &'static str,
    body: Vec<u8>,
    headers: Vec<(&'static str, &'static str)>,
    body_delay: Duration,
    prefix_len: usize,
    declared_length: Option<usize>,
}

#[tokio::test]
async fn independent_components_keep_order_and_use_complete_http_body() {
    let payload = b"<root><code>0</code></root>";
    for save in [false, true] {
        let (base, task) = server(vec![Response::new("200 OK", payload)]).await;
        let mut request = HttpRequestConfig::with_url("mixed", base);
        request.settings.save_response_body = Some(save);
        request.settings.max_saved_body_bytes = Some(1);
        request.add_assertion(JsonAssertionConfig::equals(
            "JSON code",
            "$.code",
            serde_json::json!(0),
        ));
        request.add_assertion(RegexAssertionConfig::new("raw body", "<code>0</code>"));
        request.add_assertion(XmlAssertionConfig::equals("XML code", "/root/code", "0"));
        let result = HttpSampler::new(request)
            .unwrap()
            .sample(&mut context())
            .await
            .unwrap();
        assert!(!result.success); // XML is not JSON, but other components still run.
        assert_eq!(
            result
                .assertions
                .iter()
                .map(|a| (a.name.as_str(), a.success))
                .collect::<Vec<_>>(),
            vec![("JSON code", false), ("raw body", true), ("XML code", true)]
        );
        assert_eq!(result.response_code.as_deref(), Some("200"));
        assert_eq!(result.response_message.as_deref(), Some("OK"));
        assert_eq!(result.bytes_received, payload.len() as u64);
        assert_eq!(
            result.response_body.as_deref(),
            if save { Some(b"<".as_slice()) } else { None }
        );
        task.await.unwrap();
    }
}

#[tokio::test]
async fn regex_can_check_non_json_and_passing_components_cannot_restore_http_failure() {
    for status in ["200 OK", "500 Internal Server Error"] {
        let (base, task) = server(vec![Response::new(status, b"status=ok")]).await;
        let mut request = HttpRequestConfig::with_url("plain text", base);
        request.add_assertion(RegexAssertionConfig::new("status", "^status=ok$"));
        let result = HttpSampler::new(request)
            .unwrap()
            .sample(&mut context())
            .await
            .unwrap();
        assert_eq!(result.success, status.starts_with("200"));
        assert!(result.assertions[0].success);
        task.await.unwrap();
    }
}

fn negative_assertions() -> Vec<AssertionConfig> {
    let mut xml = XmlAssertionConfig::equals("XML missing", "/missing", "");
    xml.matcher = rmeter_component::XmlMatcher::NotExists;
    vec![
        JsonAssertionConfig {
            name: "JSON missing".into(),
            path: "$.missing".into(),
            matcher: rmeter_component::JsonMatcher::NotExists,
            selection: rmeter_component::MatchSelection::One,
        }
        .into(),
        RegexAssertionConfig {
            name: "no error".into(),
            pattern: "ERROR".into(),
            invert: true,
        }
        .into(),
        xml.into(),
    ]
}

#[tokio::test]
async fn all_component_types_fail_when_http_input_is_oversized_incomplete_or_unavailable() {
    let payload = b"<root/>";
    for incomplete in [false, true] {
        let mut response = Response::new("200 OK", payload);
        if incomplete {
            response.declared_length = Some(payload.len() + 1);
        }
        let (base, task) = server(vec![response]).await;
        let mut request = HttpRequestConfig::with_url("unavailable", base);
        request.assertions = negative_assertions();
        if !incomplete {
            request.max_assertion_body_bytes = payload.len() - 1;
        }
        let result = HttpSampler::new(request)
            .unwrap()
            .sample(&mut context())
            .await
            .unwrap();
        assert_eq!(result.bytes_received, payload.len() as u64);
        assert!(!result.success);
        assert_eq!(result.assertions.len(), 3);
        for assertion in result.assertions {
            assert!(!assertion.success);
            assert!(assertion.message.unwrap().contains(if incomplete {
                "incomplete"
            } else {
                "exceeds"
            }));
        }
        task.await.unwrap();
    }
    let mut request = HttpRequestConfig::with_url("no response", "http://${missing}/");
    request.assertions = negative_assertions();
    let result = HttpSampler::new(request)
        .unwrap()
        .sample(&mut context())
        .await
        .unwrap();
    assert_eq!(result.assertions.len(), 3);
    assert!(result.assertions.iter().all(|a| !a.success));
}

#[test]
fn http_compilation_checks_regex_and_xml_configurations() {
    for config in [
        RegexAssertionConfig::new("bad config", "[").into(),
        XmlAssertionConfig::equals("bad config", "/root[", "0").into(),
        XmlAssertionConfig::equals("bad config", "/missing:root", "0").into(),
    ] {
        let mut request = HttpRequestConfig::with_url("request", "http://localhost/");
        request.assertions.push(config);
        assert!(
            HttpSampler::new(request)
                .unwrap_err()
                .message
                .contains("bad config")
        );
    }
}

impl Response {
    fn new(status: &'static str, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            body: body.into(),
            headers: vec![],
            body_delay: Duration::ZERO,
            prefix_len: 0,
            declared_length: None,
        }
    }
}

#[derive(Debug)]
struct ObservedRequest {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl ObservedRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

async fn server(responses: Vec<Response>) -> (String, JoinHandle<Vec<ObservedRequest>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(5), async move {
            let mut requests = vec![];
            for response in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buffer = vec![];
                let mut chunk = [0; 4096];
                let header_end = loop {
                    let read = stream.read(&mut chunk).await.unwrap();
                    assert_ne!(read, 0, "request closed before headers");
                    buffer.extend_from_slice(&chunk[..read]);
                    if let Some(end) = buffer.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                        break end + 4;
                    }
                };
                let text = String::from_utf8(buffer[..header_end].to_vec()).unwrap();
                let mut lines = text.split("\r\n");
                let mut first = lines.next().unwrap().split_whitespace();
                let method = first.next().unwrap().to_owned();
                let target = first.next().unwrap().to_owned();
                let headers: Vec<_> = lines
                    .filter_map(|line| line.split_once(':'))
                    .map(|(name, value)| (name.to_owned(), value.trim().to_owned()))
                    .collect();
                let length = headers
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                    .map_or(0, |(_, value)| value.parse::<usize>().unwrap());
                while buffer.len() < header_end + length {
                    let read = stream.read(&mut chunk).await.unwrap();
                    assert_ne!(read, 0, "request closed before body");
                    buffer.extend_from_slice(&chunk[..read]);
                }
                requests.push(ObservedRequest {
                    method,
                    target,
                    headers,
                    body: buffer[header_end..header_end + length].to_vec(),
                });
                let mut head = format!(
                    "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n",
                    response.status,
                    response.declared_length.unwrap_or(response.body.len())
                );
                for (name, value) in response.headers {
                    head.push_str(&format!("{name}: {value}\r\n"));
                }
                head.push_str("\r\n");
                // Timeout/cancellation tests deliberately close the client early.
                let _ = stream.write_all(head.as_bytes()).await;
                let _ = stream
                    .write_all(&response.body[..response.prefix_len])
                    .await;
                tokio::time::sleep(response.body_delay).await;
                let _ = stream
                    .write_all(&response.body[response.prefix_len..])
                    .await;
            }
            requests
        })
        .await
        .expect("local HTTP server timed out")
    });
    (base, task)
}

fn context() -> UserContext {
    UserContext::new("http", 0)
}

#[tokio::test]
async fn defaults_and_per_sample_variables_produce_encoded_requests() {
    let (base, task) = server(vec![
        Response::new("200 OK", b"ok"),
        Response::new("200 OK", b"ok"),
    ])
    .await;
    let endpoint = reqwest::Url::parse(&base).unwrap();
    let defaults = HttpRequestDefaults {
        host: endpoint.host_str().map(str::to_owned),
        port: endpoint.port(),
        path: Some("/ignored-prefix".into()),
        headers: Some(vec![
            ("X-Default".into(), "kept".into()),
            ("X-User".into(), "outer".into()),
        ]),
        query: Some(vec![
            ("default-only".into(), "must-not-leak".into()),
            ("key".into(), "old".into()),
        ]),
        ..Default::default()
    };
    let mut request = HttpRequestConfig::new("user ${id}");
    request.settings.path = Some("/users/${id}?existing=yes".into());
    request.settings.headers = Some(vec![("x-user".into(), "${id}".into())]);
    request.settings.query = Some(vec![
        ("key".into(), "${value}".into()),
        ("key".into(), "second".into()),
    ]);
    let sampler = HttpSampler::with_defaults(request, &defaults).unwrap();
    let mut context = context();
    context
        .variables
        .insert("value".into(), "a&b = 中文".into());
    for id in ["one", "two"] {
        context.variables.insert("id".into(), id.into());
        let result = sampler.sample(&mut context).await.unwrap();
        assert!(result.success);
        assert_eq!(result.label, format!("user {id}"));
        assert_eq!(result.bytes_received, 2);
        assert!(result.response_body.is_none());
    }
    let requests = task.await.unwrap();
    for (request, id) in requests.iter().zip(["one", "two"]) {
        assert_eq!(request.method, "GET");
        let url = reqwest::Url::parse(&format!("{base}{}", request.target)).unwrap();
        assert_eq!(url.path(), format!("/users/{id}"));
        assert_eq!(
            url.query_pairs().collect::<Vec<_>>(),
            vec![
                ("existing".into(), "yes".into()),
                ("key".into(), "a&b = 中文".into()),
                ("key".into(), "second".into()),
            ]
        );
        assert_eq!(request.header("x-default"), None);
        assert_eq!(request.header("x-user"), Some(id));
    }
}

#[tokio::test]
async fn sends_text_binary_forms_and_safely_expanded_json() {
    let responses = (0..4)
        .map(|_| Response::new("201 Created", b"created"))
        .collect();
    let (base, task) = server(responses).await;
    let value = "quoted\" value & 中文";
    let bodies = [
        HttpBody::Text("${value}".into()),
        HttpBody::Bytes(vec![0, 255, 42]),
        HttpBody::Form(vec![
            ("key".into(), "${value}".into()),
            ("key".into(), "second".into()),
        ]),
        HttpBody::Json(serde_json::json!({"nested": ["${value}", 42]})),
    ];
    let mut context = context();
    context.variables.insert("value".into(), value.into());
    for body in bodies {
        let mut request = HttpRequestConfig::with_url("post", &base);
        request.method = HttpMethod::POST;
        request.body = body;
        let sampler = HttpSampler::new(request).unwrap();
        let result = sampler.sample(&mut context).await.unwrap();
        assert!(result.success);
        assert_eq!(result.response_code.as_deref(), Some("201"));
        assert!(result.bytes_sent > 0);
    }
    let requests = task.await.unwrap();
    assert!(requests.iter().all(|request| request.method == "POST"));
    assert_eq!(requests[0].body, value.as_bytes());
    assert_eq!(
        requests[0].header("content-type"),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(requests[1].body, vec![0, 255, 42]);
    assert_eq!(
        requests[2].header("content-type"),
        Some("application/x-www-form-urlencoded")
    );
    let form = String::from_utf8(requests[2].body.clone()).unwrap();
    let url = reqwest::Url::parse(&format!("http://localhost/?{form}")).unwrap();
    assert_eq!(
        url.query_pairs().collect::<Vec<_>>(),
        vec![
            ("key".into(), value.into()),
            ("key".into(), "second".into())
        ]
    );
    assert_eq!(requests[3].header("content-type"), Some("application/json"));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&requests[3].body).unwrap(),
        serde_json::json!({"nested": [value, 42]})
    );
}

#[tokio::test]
async fn failed_http_status_keeps_metadata_and_truncates_only_saved_body() {
    let (base, task) = server(vec![Response::new(
        "500 Internal Server Error",
        b"full error response",
    )])
    .await;
    let mut request = HttpRequestConfig::with_url("failure", &base);
    request.body = HttpBody::Bytes(vec![1, 2, 3]);
    request.settings.save_response_body = Some(true);
    request.settings.max_saved_body_bytes = Some(4);
    let result = HttpSampler::new(request)
        .unwrap()
        .sample(&mut context())
        .await
        .unwrap();
    assert!(!result.success);
    assert_eq!(result.label, "failure");
    assert_eq!(result.response_code.as_deref(), Some("500"));
    assert_eq!(result.bytes_sent, 3);
    assert_eq!(result.bytes_received, 19);
    assert_eq!(result.response_body.as_deref(), Some(b"full".as_slice()));
    task.await.unwrap();
}

#[tokio::test]
async fn consumes_unsaved_body_and_includes_body_time() {
    let mut response = Response::new("200 OK", b"delayed body");
    response.body_delay = Duration::from_millis(50);
    let (base, task) = server(vec![response]).await;
    let sampler = HttpSampler::new(HttpRequestConfig::with_url("delayed", &base)).unwrap();
    let result = sampler.sample(&mut context()).await.unwrap();
    assert!(result.success);
    assert_eq!(result.bytes_received, 12);
    assert!(result.response_body.is_none());
    assert!(result.elapsed >= Duration::from_millis(40));
    task.await.unwrap();
}

#[tokio::test]
async fn follows_redirects_or_returns_redirect_status_when_disabled() {
    for follow in [false, true] {
        let mut redirect = Response::new("302 Found", b"");
        redirect.headers.push(("Location", "/final"));
        let mut responses = vec![redirect];
        if follow {
            responses.push(Response::new("200 OK", b"final"));
        }
        let (base, task) = server(responses).await;
        let mut request = HttpRequestConfig::with_url("redirect", format!("{base}/start"));
        request.settings.follow_redirects = Some(follow);
        let result = HttpSampler::new(request)
            .unwrap()
            .sample(&mut context())
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(
            result.response_code.as_deref(),
            Some(if follow { "200" } else { "302" })
        );
        assert_eq!(result.bytes_received, if follow { 5 } else { 0 });
        let requests = task.await.unwrap();
        assert_eq!(requests.len(), if follow { 2 } else { 1 });
        if follow {
            assert_eq!(requests[1].target, "/final");
        }
    }
}

#[tokio::test]
async fn missing_variables_and_invalid_expanded_headers_are_failed_samples() {
    let sampler =
        HttpSampler::new(HttpRequestConfig::with_url("missing", "http://${host}/")).unwrap();
    let result = sampler.sample(&mut context()).await.unwrap();
    assert!(!result.success);
    assert_eq!(result.label, "missing");
    assert!(
        result
            .response_message
            .as_deref()
            .unwrap()
            .contains("undefined variable")
    );
    let mut request = HttpRequestConfig::with_url("header", "http://localhost/");
    request.settings.headers = Some(vec![("X-Value".into(), "${value}".into())]);
    let sampler = HttpSampler::new(request).unwrap();
    let mut context = context();
    context
        .variables
        .insert("value".into(), "bad\r\nInjected: value".into());
    let result = sampler.sample(&mut context).await.unwrap();
    assert!(!result.success);
    assert_eq!(result.label, "header");
    assert!(
        result
            .response_message
            .as_deref()
            .unwrap()
            .starts_with("InvalidConfig:")
    );
}

#[tokio::test]
async fn transport_failure_keeps_request_label() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = listener.local_addr().unwrap();
    drop(listener);
    let sampler = HttpSampler::new(HttpRequestConfig::with_url(
        "connection",
        format!("http://{endpoint}/"),
    ))
    .unwrap();
    let result = sampler.sample(&mut context()).await.unwrap();
    assert!(!result.success);
    assert_eq!(result.label, "connection");
    assert!(result.response_code.is_none());
    assert!(
        result
            .response_message
            .as_deref()
            .unwrap()
            .starts_with("Transport:")
    );
}

#[tokio::test]
async fn whole_request_timeout_includes_body_and_keeps_partial_response() {
    let mut response = Response::new("200 OK", b"partial then delayed");
    response.prefix_len = 7;
    response.body_delay = Duration::from_millis(500);
    let (base, task) = server(vec![response]).await;
    let mut request = HttpRequestConfig::with_url("timeout", &base);
    request.settings.request_timeout = Setting::Value(Some(Duration::from_millis(100)));
    request.settings.save_response_body = Some(true);
    let result = HttpSampler::new(request)
        .unwrap()
        .sample(&mut context())
        .await
        .unwrap();
    assert!(!result.success);
    assert_eq!(result.label, "timeout");
    assert_eq!(result.response_code.as_deref(), Some("200"));
    assert_eq!(result.bytes_received, 7);
    assert_eq!(result.response_body.as_deref(), Some(b"partial".as_slice()));
    assert!(
        result
            .response_message
            .as_deref()
            .unwrap()
            .starts_with("Timeout:")
    );
    task.await.unwrap();
}

#[tokio::test]
async fn incomplete_body_is_a_failed_sample_even_with_success_status() {
    let mut response = Response::new("200 OK", b"short");
    response.declared_length = Some(100);
    let (base, task) = server(vec![response]).await;
    let sampler = HttpSampler::new(HttpRequestConfig::with_url("incomplete", &base)).unwrap();
    let result = sampler.sample(&mut context()).await.unwrap();
    assert!(!result.success);
    assert_eq!(result.response_code.as_deref(), Some("200"));
    assert!(
        result
            .response_message
            .as_deref()
            .unwrap()
            .starts_with("Transport:")
    );
    task.await.unwrap();
}

#[tokio::test]
async fn compiled_http_components_run_in_engine_with_error_policy() {
    let (base, task) = server(vec![Response::new("503 Service Unavailable", b"busy")]).await;
    let children = compile_http_nodes(
        vec![
            HttpNode::Request(HttpRequestConfig::with_url("failed HTTP", &base)),
            HttpNode::Request(HttpRequestConfig::with_url("must be skipped", &base)),
        ],
        &HttpRequestDefaults::default(),
    )
    .unwrap();
    let plan = TestPlan {
        name: "http engine".into(),
        variables: HashMap::new(),
        groups: vec![ThreadGroup {
            name: "http".into(),
            users: 1,
            ramp_up: Duration::ZERO,
            iterations: LoopCount::Finite(2),
            max_duration: None,
            on_error: ErrorPolicy::StopUser,
            children,
        }],
    };
    let engine = Engine::new(plan);
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let (result, events) = tokio::join!(engine.run(sender), async move {
        let mut events = vec![];
        while let Some(event) = receiver.recv().await {
            events.push(event);
        }
        events
    });
    result.unwrap();
    assert_eq!(events.len(), 3);
    assert!(matches!(events[0], RunEvent::Started { .. }));
    let RunEvent::SampleCompleted(sample) = &events[1] else {
        panic!("expected HTTP sample")
    };
    assert_eq!(sample.result.label, "failed HTTP");
    assert_eq!(sample.result.response_code.as_deref(), Some("503"));
    assert!(!sample.result.success);
    assert!(matches!(events[2], RunEvent::Finished { .. }));
    task.await.unwrap();
}

#[tokio::test]
async fn compiled_requests_reuse_the_same_connection() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(5), async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut requests = vec![];
            for _ in 0..2 {
                let mut buffer = vec![];
                let mut chunk = [0; 4096];
                while !buffer.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                    let read = stream.read(&mut chunk).await.unwrap();
                    assert_ne!(read, 0, "compatible requests must reuse the connection");
                    buffer.extend_from_slice(&chunk[..read]);
                }
                requests.push(String::from_utf8(buffer).unwrap());
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .await
                    .unwrap();
            }
            requests
        })
        .await
        .expect("requests opened separate connections instead of reusing the pool")
    });
    let nodes = compile_http_nodes(
        vec![
            HttpNode::Request(HttpRequestConfig::with_url("one", format!("{base}/one"))),
            HttpNode::Request(HttpRequestConfig::with_url("two", format!("{base}/two"))),
        ],
        &HttpRequestDefaults::default(),
    )
    .unwrap();
    let mut context = context();
    let mut cursor = rmeter_core::controller::ExecutionCursor::new(&nodes);
    loop {
        match cursor.step(&mut context).unwrap() {
            rmeter_core::controller::CursorStep::Sample(sampler) => {
                assert!(sampler.sample(&mut context).await.unwrap().success);
            }
            rmeter_core::controller::CursorStep::Yield => tokio::task::yield_now().await,
            rmeter_core::controller::CursorStep::Done => break,
        }
    }
    let requests = server.await.unwrap();
    assert!(requests[0].starts_with("GET /one "));
    assert!(requests[1].starts_with("GET /two "));
}

#[tokio::test]
async fn engine_cancellation_interrupts_an_in_flight_http_request() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let (entered, pending_request) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        tokio::time::timeout(Duration::from_secs(5), async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buffer = vec![];
            let mut chunk = [0; 4096];
            while !buffer.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let read = stream.read(&mut chunk).await.unwrap();
                assert_ne!(read, 0);
                buffer.extend_from_slice(&chunk[..read]);
            }
            entered.send(()).unwrap();
            // With no response, the only way to finish is cancellation.
            match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => {}
                Ok(_) => panic!("expected the cancelled request to close"),
            }
        })
        .await
        .expect("cancelled HTTP request kept the connection open")
    });
    let mut request = HttpRequestConfig::with_url("pending", url);
    request.settings.request_timeout = Setting::Value(None);
    let sampler = HttpSampler::new(request).unwrap();
    let plan = TestPlan {
        name: "cancel HTTP".into(),
        variables: HashMap::new(),
        groups: vec![ThreadGroup {
            name: "http".into(),
            users: 1,
            ramp_up: Duration::ZERO,
            iterations: LoopCount::Finite(1),
            max_duration: None,
            on_error: ErrorPolicy::Continue,
            children: vec![rmeter_core::plan::Node::Sample(sampler)],
        }],
    };
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let task = tokio::spawn(async move { Engine::new(plan).run(sender).await });
    assert!(matches!(
        receiver.recv().await,
        Some(RunEvent::Started { .. })
    ));
    tokio::time::timeout(Duration::from_secs(5), pending_request)
        .await
        .unwrap()
        .unwrap();
    drop(receiver);
    let result = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        result,
        Err(rmeter_core::engine::EngineError::EventsClosed)
    ));
    server.await.unwrap();
}

#[tokio::test]
async fn conditional_http_tree_preserves_defaults_scopes_and_runtime_conditions() {
    let (base, task) = server(vec![
        Response::new("200 OK", b"if"),
        Response::new("500 Internal Server Error", b"stop while"),
        Response::new("200 OK", b"after"),
    ])
    .await;
    let endpoint = reqwest::Url::parse(&base).unwrap();
    let defaults = HttpRequestDefaults {
        host: endpoint.host_str().map(str::to_owned),
        port: endpoint.port(),
        headers: Some(vec![("X-Parent".into(), "parent".into())]),
        ..Default::default()
    };
    let children = compile_http_nodes(
        vec![HttpNode::Controller {
            name: "generic".into(),
            children: vec![
                HttpNode::If {
                    name: "if".into(),
                    condition: "${enabled}".into(),
                    evaluate_all: false,
                    children: vec![
                        HttpNode::Request(HttpRequestConfig::new("if")),
                        HttpNode::Defaults(HttpRequestDefaults {
                            path: Some("/if".into()),
                            headers: Some(vec![("X-Scope".into(), "if".into())]),
                            ..Default::default()
                        }),
                    ],
                },
                HttpNode::While {
                    name: "while".into(),
                    condition: "LAST".into(),
                    children: vec![
                        HttpNode::Request(HttpRequestConfig::new("while")),
                        HttpNode::Defaults(HttpRequestDefaults {
                            path: Some("/while".into()),
                            headers: Some(vec![("X-Scope".into(), "while".into())]),
                            ..Default::default()
                        }),
                    ],
                },
                HttpNode::Request(HttpRequestConfig::new("after")),
            ],
        }],
        &defaults,
    )
    .unwrap();
    let engine = Engine::new(TestPlan {
        name: "conditional HTTP".into(),
        variables: HashMap::from([("enabled".into(), "true".into())]),
        groups: vec![ThreadGroup {
            name: "HTTP".into(),
            users: 1,
            ramp_up: Duration::ZERO,
            iterations: LoopCount::Finite(1),
            max_duration: None,
            on_error: ErrorPolicy::Continue,
            children,
        }],
    });
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let (result, events) = tokio::join!(engine.run(sender), async {
        let mut events = Vec::new();
        while let Some(event) = receiver.recv().await {
            events.push(event);
        }
        events
    });
    result.unwrap();
    let samples: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            RunEvent::SampleCompleted(sample) => {
                Some((sample.result.label.as_str(), sample.result.success))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        samples,
        vec![("if", true), ("while", false), ("after", true)]
    );
    let requests = task.await.unwrap();
    assert_eq!(
        requests
            .iter()
            .map(|request| request.target.as_str())
            .collect::<Vec<_>>(),
        vec!["/if", "/while", "/"]
    );
    assert!(
        requests
            .iter()
            .all(|request| request.header("X-Parent") == Some("parent"))
    );
    assert_eq!(requests[0].header("X-Scope"), Some("if"));
    assert_eq!(requests[1].header("X-Scope"), Some("while"));
    assert_eq!(requests[2].header("X-Scope"), None);
}

#[tokio::test]
async fn json_assertions_use_complete_body_with_saving_disabled_or_truncated() {
    let payload = br#"{"code":0,"data":{"name":"user-42"}}"#;
    for save in [false, true] {
        let (base, task) = server(vec![Response::new("200 OK", payload)]).await;
        let mut request = HttpRequestConfig::with_url("asserted", &base);
        request.settings.save_response_body = Some(save);
        request.settings.max_saved_body_bytes = Some(1);
        request.assertions = vec![
            JsonAssertionConfig::equals("code", "$.code", serde_json::json!(0)).into(),
            JsonAssertionConfig {
                name: "name".into(),
                path: "$.data.name".into(),
                matcher: rmeter_component::JsonMatcher::Regex("^user-[0-9]+$".into()),
                selection: rmeter_component::MatchSelection::One,
            }
            .into(),
        ];
        let result = HttpSampler::new(request)
            .unwrap()
            .sample(&mut context())
            .await
            .unwrap();
        assert!(result.success);
        assert_eq!(result.assertions.len(), 2);
        assert!(result.assertions.iter().all(|assertion| assertion.success));
        assert_eq!(result.bytes_received, payload.len() as u64);
        assert_eq!(
            result.response_body.as_deref(),
            if save { Some(b"{".as_slice()) } else { None }
        );
        task.await.unwrap();
    }
}

#[tokio::test]
async fn failed_json_assertion_preserves_http_status_and_reports_details() {
    let (base, task) = server(vec![Response::new("200 OK", br#"{"code":7}"#)]).await;
    let mut request = HttpRequestConfig::with_url("business failure", &base);
    request.add_assertion(JsonAssertionConfig::equals(
        "code must be zero",
        "$.code",
        serde_json::json!(0),
    ));
    let result = HttpSampler::new(request)
        .unwrap()
        .sample(&mut context())
        .await
        .unwrap();
    assert!(!result.success);
    assert_eq!(result.label, "business failure");
    assert_eq!(result.response_code.as_deref(), Some("200"));
    assert_eq!(result.response_message.as_deref(), Some("OK"));
    assert_eq!(result.assertions[0].name, "code must be zero");
    assert!(
        result.assertions[0]
            .message
            .as_deref()
            .unwrap()
            .contains("$.code")
    );
    task.await.unwrap();
}

#[tokio::test]
async fn assertion_body_limit_is_independent_and_still_drains_response() {
    let payload = br#"{"code":0}"#;
    for limit in [payload.len(), payload.len() - 1] {
        let (base, task) = server(vec![Response::new("200 OK", payload)]).await;
        let mut request = HttpRequestConfig::with_url("limit", &base);
        request.add_assertion(JsonAssertionConfig::equals(
            "code",
            "$.code",
            serde_json::json!(0),
        ));
        request.max_assertion_body_bytes = limit;
        request.settings.save_response_body = Some(true);
        let result = HttpSampler::new(request)
            .unwrap()
            .sample(&mut context())
            .await
            .unwrap();
        assert_eq!(result.success, limit == payload.len());
        assert_eq!(result.bytes_received, payload.len() as u64);
        assert_eq!(result.response_body.as_deref(), Some(payload.as_slice()));
        if limit < payload.len() {
            assert!(
                result.assertions[0]
                    .message
                    .as_deref()
                    .unwrap()
                    .contains("exceeds")
            );
        }
        task.await.unwrap();
    }
}

#[tokio::test]
async fn invalid_or_incomplete_json_cannot_pass_assertions() {
    let mut incomplete = Response::new("200 OK", br#"{"code":0}"#);
    incomplete.declared_length = Some(incomplete.body.len() + 1);
    for response in [Response::new("200 OK", b"not JSON"), incomplete] {
        let (base, task) = server(vec![response]).await;
        let mut request = HttpRequestConfig::with_url("invalid body", &base);
        request.add_assertion(JsonAssertionConfig::equals(
            "code",
            "$.code",
            serde_json::json!(0),
        ));
        let result = HttpSampler::new(request)
            .unwrap()
            .sample(&mut context())
            .await
            .unwrap();
        assert!(!result.success);
        assert!(!result.assertions[0].success);
        assert_eq!(result.response_code.as_deref(), Some("200"));
        task.await.unwrap();
    }
}

#[tokio::test]
async fn failed_request_records_unavailable_assertion_input() {
    let mut request = HttpRequestConfig::with_url("missing variable", "http://${missing}/");
    request.add_assertion(JsonAssertionConfig::equals(
        "code",
        "$.code",
        serde_json::json!(0),
    ));
    let result = HttpSampler::new(request)
        .unwrap()
        .sample(&mut context())
        .await
        .unwrap();
    assert!(!result.success);
    assert_eq!(result.assertions.len(), 1);
    assert!(!result.assertions[0].success);
    assert!(
        result.assertions[0]
            .message
            .as_deref()
            .unwrap()
            .contains("complete response")
    );
}

#[test]
fn http_sampler_validates_assertion_configuration_before_running() {
    let mut request = HttpRequestConfig::with_url("invalid assertion", "http://localhost/");
    request.add_assertion(JsonAssertionConfig::equals(
        "invalid path",
        "$.items[",
        serde_json::json!(0),
    ));
    assert!(
        HttpSampler::new(request)
            .unwrap_err()
            .message
            .contains("invalid path")
    );
}

#[tokio::test]
async fn json_assertion_failures_trigger_all_engine_error_policies() {
    for (policy, expected) in [
        (
            ErrorPolicy::Continue,
            vec![("asserted", 0), ("after", 0), ("asserted", 1), ("after", 1)],
        ),
        (
            ErrorPolicy::NextIteration,
            vec![("asserted", 0), ("asserted", 1)],
        ),
        (ErrorPolicy::StopUser, vec![("asserted", 0)]),
        (ErrorPolicy::StopTest, vec![("asserted", 0)]),
    ] {
        let (base, task) = server(
            expected
                .iter()
                .map(|_| Response::new("200 OK", br#"{"code":7}"#))
                .collect(),
        )
        .await;
        let mut request = HttpRequestConfig::with_url("asserted", &base);
        request.add_assertion(JsonAssertionConfig::equals(
            "code",
            "$.code",
            serde_json::json!(0),
        ));
        let children = compile_http_nodes(
            vec![
                HttpNode::Request(request),
                HttpNode::Request(HttpRequestConfig::with_url("after", &base)),
            ],
            &HttpRequestDefaults::default(),
        )
        .unwrap();
        let engine = Engine::new(TestPlan {
            name: "assertion policy".into(),
            variables: HashMap::new(),
            groups: vec![ThreadGroup {
                name: "http".into(),
                users: 1,
                ramp_up: Duration::ZERO,
                iterations: LoopCount::Finite(2),
                max_duration: None,
                on_error: policy,
                children,
            }],
        });
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
        let (result, samples) = tokio::join!(engine.run(sender), async {
            let mut samples = vec![];
            while let Some(event) = receiver.recv().await {
                if let RunEvent::SampleCompleted(sample) = event {
                    samples.push(sample);
                }
            }
            samples
        });
        result.unwrap();
        assert_eq!(
            samples
                .iter()
                .map(|sample| (sample.result.label.as_str(), sample.iteration))
                .collect::<Vec<_>>(),
            expected
        );
        for sample in samples
            .iter()
            .filter(|sample| sample.result.label == "asserted")
        {
            assert!(!sample.result.success);
            assert!(!sample.result.assertions[0].success);
            assert_eq!(sample.result.response_code.as_deref(), Some("200"));
        }
        task.await.unwrap();
    }
}

#[tokio::test]
async fn assertion_failure_is_visible_to_if_and_while_before_the_next_sample() {
    let (base, task) = server(vec![
        Response::new("200 OK", br#"{"code":7}"#),
        Response::new("200 OK", b"after"),
    ])
    .await;
    let mut request = HttpRequestConfig::with_url("asserted", &base);
    request.add_assertion(JsonAssertionConfig::equals(
        "code",
        "$.code",
        serde_json::json!(0),
    ));
    let children = compile_http_nodes(
        vec![
            HttpNode::If {
                name: "all".into(),
                condition: rmeter_core::controller::Condition::LastSampleOk,
                evaluate_all: true,
                children: vec![
                    HttpNode::Request(request),
                    HttpNode::Request(HttpRequestConfig::with_url("skip IF", &base)),
                ],
            },
            HttpNode::While {
                name: "LAST".into(),
                condition: "LAST".into(),
                children: vec![HttpNode::Request(HttpRequestConfig::with_url(
                    "skip While",
                    &base,
                ))],
            },
            HttpNode::Request(HttpRequestConfig::with_url("after", &base)),
        ],
        &HttpRequestDefaults::default(),
    )
    .unwrap();
    let engine = Engine::new(TestPlan {
        name: "assertions and controllers".into(),
        variables: HashMap::new(),
        groups: vec![ThreadGroup {
            name: "http".into(),
            users: 1,
            ramp_up: Duration::ZERO,
            iterations: LoopCount::Finite(1),
            max_duration: None,
            on_error: ErrorPolicy::Continue,
            children,
        }],
    });
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let (result, samples) = tokio::join!(engine.run(sender), async {
        let mut samples = Vec::new();
        while let Some(event) = receiver.recv().await {
            if let RunEvent::SampleCompleted(sample) = event {
                samples.push(sample);
            }
        }
        samples
    });
    result.unwrap();
    assert_eq!(
        samples
            .iter()
            .map(|sample| sample.result.label.as_str())
            .collect::<Vec<_>>(),
        vec!["asserted", "after"]
    );
    task.await.unwrap();
}
