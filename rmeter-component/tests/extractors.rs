use rmeter_component::extractor::{ExtractorConfig, ExtractorKind as K, ExtractorSettings};
use rmeter_component::{HttpRequestConfig, HttpSampler, JavaScriptConfig};
use rmeter_core::{
    context::UserContext,
    sampler::{SampleResult, Sampler},
};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn config(kind: K, expression: &str) -> ExtractorConfig {
    ExtractorConfig {
        kind,
        settings: ExtractorSettings::new("extract", "value", expression),
    }
}
async fn server(body: Vec<u8>) -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = stream.read(&mut buffer).await.unwrap();
            assert!(n > 0);
            request.extend_from_slice(&buffer[..n]);
            assert!(request.len() <= 16384);
        }
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
    });
    (url, task)
}
async fn sample(config: ExtractorConfig, body: &[u8], user: &mut UserContext) -> SampleResult {
    let (url, task) = server(body.to_vec()).await;
    let mut http = HttpRequestConfig::with_url("parent", url);
    http.settings.save_response_body = Some(false);
    http.add_post_processor(config);
    let result = HttpSampler::new(http).unwrap().sample(user).await.unwrap();
    task.await.unwrap();
    result
}
#[tokio::test]
async fn all_five_extractors_read_full_body_without_saved_preview() {
    for (kind, expression, body, expected) in [
        (
            K::Jsonpath,
            "$.items[*].id",
            r#"{"items":[{"id":"a"},{"id":"b"}]}"#,
            "a",
        ),
        (
            K::Jmespath,
            "items[?active].id",
            r#"{"items":[{"id":"a","active":true},{"id":"b","active":false}]}"#,
            "a",
        ),
        (
            K::Regex,
            r#"id="(\w+)""#,
            r#"<item id="a"/><item id="b"/>"#,
            "a",
        ),
        (
            K::Xpath,
            "//item/text()",
            "<root><item>a</item><item>b</item></root>",
            "a",
        ),
        (
            K::Xpath2,
            "for $n in //item return upper-case(string($n))",
            "<root><item>a</item><item>b</item></root>",
            "A",
        ),
    ] {
        let mut user = UserContext::new("g", 0);
        let result = sample(config(kind, expression), body.as_bytes(), &mut user).await;
        assert!(result.success, "{kind:?}: {:?}", result.response_message);
        assert!(result.response_body.is_none());
        assert_eq!(user.variables["value"], expected);
        assert_eq!(user.script_logs.entries[0].source, "extract");
    }
}
#[tokio::test]
async fn all_matches_defaults_random_and_stale_values_follow_jmeter_names() {
    let mut c = config(K::Jsonpath, "$.items[*]");
    c.settings.match_number = -1;
    let mut user = UserContext::new("g", 0);
    assert!(
        sample(c.clone(), br#"{"items":["a",null,3,{"id":4}]}"#, &mut user)
            .await
            .success
    );
    assert_eq!(user.variables["value_matchNr"], "4");
    assert_eq!(user.variables["value_2"], "null");
    assert_eq!(user.variables["value_4"], r#"{"id":4}"#);
    user.variables.insert("value_custom".into(), "keep".into());
    c.settings.match_number = 2;
    assert!(
        sample(c.clone(), br#"{"items":["x","y"]}"#, &mut user)
            .await
            .success
    );
    assert_eq!(user.variables["value"], "y");
    assert!(!user.variables.contains_key("value_4"));
    c.settings.match_number = 0;
    assert!(
        sample(c.clone(), br#"{"items":["x","y"]}"#, &mut user)
            .await
            .success
    );
    assert!(["x", "y"].contains(&user.variables["value"].as_str()));
    c.settings.default_value = Some("missing".into());
    assert!(
        sample(c.clone(), br#"{"items":[]}"#, &mut user)
            .await
            .success
    );
    assert_eq!(user.variables["value"], "missing");
    assert_eq!(user.variables["value_matchNr"], "0");
    assert_eq!(user.variables["value_custom"], "keep");
    c.settings.default_value = None;
    assert!(sample(c, br#"{"items":[]}"#, &mut user).await.success);
    assert!(!user.variables.contains_key("value"));
}
#[tokio::test]
async fn regex_templates_and_namespace_aware_xml_queries() {
    let mut c = config(K::Regex, r"(\w+)=(\d+)");
    c.settings.match_number = -1;
    c.settings.template = "$2$:$1$".into();
    let mut user = UserContext::new("g", 0);
    assert!(sample(c, b"a=10 b=20", &mut user).await.success);
    assert_eq!(user.variables["value_2"], "20:b");
    assert_eq!(user.variables["value_2_g1"], "b");
    assert_eq!(user.variables["value_2_g0"], "b=20");
    for kind in [K::Xpath, K::Xpath2] {
        let mut c = config(kind, "//n:item/@id");
        c.settings.namespaces.insert("n".into(), "urn:test".into());
        c.settings.match_number = -1;
        let result = sample(
            c,
            br#"<root xmlns="urn:test"><item id="one"/><item id="two"/></root>"#,
            &mut user,
        )
        .await;
        assert!(result.success, "{:?}", result.response_message);
        assert_eq!(user.variables["value_1"], "one");
        assert_eq!(user.variables["value_2"], "two");
        assert!(!user.variables.contains_key("value_2_g1"));
    }
    let mut c = config(
        K::Xpath2,
        "string-join(for $i in //item return upper-case(string($i)), ',')",
    );
    c.settings.namespaces.insert("".into(), "urn:test".into());
    assert!(
        sample(
            c,
            br#"<root xmlns="urn:test"><item>a</item><item>b</item></root>"#,
            &mut user
        )
        .await
        .success
    );
    assert_eq!(user.variables["value"], "A,B");
}
#[tokio::test]
async fn mixed_processors_preserve_order_and_outputs_reach_next_script() {
    let (url, task) = server(br#"{"token":"abc"}"#.to_vec()).await;
    let mut http = HttpRequestConfig::with_url("ordered", url);
    let mut js = JavaScriptConfig::new("before", "vars.put('phase','before');");
    js.timeout = Duration::from_secs(2);
    http.add_post_processor(js);
    http.add_post_processor(config(K::Jsonpath, "$.token"));
    let mut js = JavaScriptConfig::new(
        "after",
        "if(vars.get('phase') !== 'before' || vars.get('value') !== 'abc') throw new Error('wrong order'); vars.put('result', vars.get('value'));",
    );
    js.timeout = Duration::from_secs(2);
    http.add_post_processor(js);
    let mut user = UserContext::new("g", 0);
    assert!(
        HttpSampler::new(http)
            .unwrap()
            .sample(&mut user)
            .await
            .unwrap()
            .success
    );
    assert_eq!(user.variables["result"], "abc");
    task.await.unwrap();
}
#[tokio::test]
async fn invalid_inputs_and_limits_fail_parent_and_clear_old_outputs() {
    for (mut c, body) in [
        (config(K::Jsonpath, "$.id"), b"bad JSON".as_slice()),
        (config(K::Xpath, "//item"), b"<unclosed>".as_slice()),
        (
            config(K::Xpath2, "//item"),
            b"<!DOCTYPE r [<!ENTITY x SYSTEM 'file:///nope'>]><r>&x;</r>".as_slice(),
        ),
        (config(K::Regex, "(x)"), &[0xff, 0xfe]),
    ] {
        c.settings.default_value = Some("fallback".into());
        let mut user = UserContext::new("g", 0);
        user.variables.insert("value_99".into(), "stale".into());
        let result = sample(c, body, &mut user).await;
        assert!(!result.success);
        assert_eq!(result.response_code.as_deref(), Some("200"));
        assert_eq!(user.variables["value_matchNr"], "0");
        assert_eq!(user.variables["value"], "fallback");
        assert!(!user.variables.contains_key("value_99"));
    }
    for limit_matches in [true, false] {
        let mut c = config(K::Jsonpath, "$.items[*]");
        c.settings.match_number = -1;
        if limit_matches {
            c.settings.max_matches = 1;
        } else {
            c.settings.max_variable_bytes = 100;
        }
        let mut user = UserContext::new("g", 0);
        let result = sample(
            c,
            format!("{{\"items\":[\"{}\",\"b\"]}}", "x".repeat(200)).as_bytes(),
            &mut user,
        )
        .await;
        assert!(!result.success);
        assert!(!user.variables.contains_key("value_1"));
    }
}
#[tokio::test]
async fn xpath2_large_queries_timeout_and_cancellation_releases_worker() {
    let mut c = config(K::Xpath2, "for $i in 1 to 1000000000 return string($i)");
    c.settings.timeout_ms = 10;
    let mut user = UserContext::new("g", 0);
    let result = sample(c.clone(), b"<r/>", &mut user).await;
    assert!(!result.success);
    assert!(result.response_message.unwrap().contains("Timeout"));
    c.settings.timeout_ms = 60000;
    let extractor = rmeter_component::extractor::Extractor::new(c).unwrap();
    // Direct cancellation of HTTP processing; no long-lived worker slot may remain.
    let (url, server) = server(b"<r/>".to_vec()).await;
    let mut http = HttpRequestConfig::with_url("cancel", url);
    http.add_post_processor(extractor.config().clone());
    let sampler = HttpSampler::new(http).unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(10), sampler.sample(&mut user))
            .await
            .is_err()
    );
    server.await.unwrap();
    assert!(
        sample(config(K::Jsonpath, "$.id"), br#"{"id":42}"#, &mut user)
            .await
            .success
    );
    assert_eq!(user.variables["value"], "42");
}
#[test]
fn bad_expressions_and_templates_fail_validation_without_http_requests() {
    for c in [
        config(K::Jsonpath, "$["),
        config(K::Jmespath, "["),
        config(K::Xpath, "//missing:item"),
        config(K::Xpath2, "for $x in return"),
        config(K::Regex, "["),
    ] {
        assert!(rmeter_component::extractor::Extractor::new(c).is_err());
    }
    let mut c = config(K::Regex, "(a)");
    c.settings.template = "$2$".into();
    assert!(rmeter_component::extractor::Extractor::new(c).is_err());
}
