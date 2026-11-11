use rmeter_component::{
    HttpRequestConfig, HttpSampler, JavaScriptConfig, JavaScriptSampler, JdbcConnection,
    JdbcConnectionConfig, JdbcSampler, JdbcSamplerConfig, JsonAssertionConfig,
};
use rmeter_core::{context::UserContext, sampler::Sampler};
use serde_json::json;
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn processor(name: &str, script: &str) -> JavaScriptConfig {
    let mut config = JavaScriptConfig::new(name, script);
    config.timeout = Duration::from_secs(5);
    config
}

async fn http_response(response: Vec<u8>) -> (String, tokio::task::JoinHandle<()>) {
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
        }
        stream.write_all(&response).await.unwrap();
    });
    (url, task)
}

fn response(body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}

#[tokio::test]
async fn http_processors_receive_complete_readonly_results_and_commit_in_order() {
    let body = br#"{"token":"abc"}"#;
    for save in [false, true] {
        let (url, server) = http_response(response(body)).await;
        let mut config = HttpRequestConfig::with_url("login", url);
        config.settings.save_response_body = Some(save);
        config.settings.max_saved_body_bytes = Some(2);
        config.max_assertion_body_bytes = 1; // No assertions, no influence on post input.
        config.post_processors = vec![
            processor(
                "extract",
                r#"
                if (!prev.success || prev.responseCode !== '200' || prev.label !== 'login'
                    || prev.elapsedMs < 0 || prev.bodyUnavailableReason !== null) throw new Error('metadata');
                const original = prev.body;
                prev.body = 'changed'; prev = {}; delete prev.success;
                if (prev.body !== original) throw new Error('mutable result');
                vars.set('token', JSON.parse(prev.body).token);
            "#,
            ),
            processor(
                "derive",
                "vars.set('authorization', `Bearer ${vars.get('token')}`);",
            ),
        ];
        let mut user = UserContext::new("g", 0);
        let result = HttpSampler::new(config)
            .unwrap()
            .sample(&mut user)
            .await
            .unwrap();
        assert!(result.success, "{:?}", result.response_message);
        assert_eq!(user.variables["authorization"], "Bearer abc");
        assert_eq!(result.bytes_received, body.len() as u64);
        assert_eq!(result.response_body, save.then(|| body[..2].to_vec()));
        server.await.unwrap();
        // The same user's later standalone script must not inherit prev.
        assert!(
            JavaScriptSampler::new(processor(
                "standalone",
                "if (typeof prev !== 'undefined') throw new Error('leaked prev');"
            ))
            .unwrap()
            .sample(&mut user)
            .await
            .unwrap()
            .success
        );
    }
}

#[tokio::test]
async fn processors_run_before_assertions_and_cannot_restore_parent_success() {
    let (url, server) = http_response(response(br#"{"code":1}"#)).await;
    let mut config = HttpRequestConfig::with_url("asserted", url);
    config.add_assertion(JsonAssertionConfig::equals("code", "$.code", json!(0)));
    config.post_processors = vec![processor(
        "inspect failure",
        r#"
        if (prev.success !== true) throw new Error('assertions ran too early');
        if (JSON.parse(prev.body).code !== 1) throw new Error('body');
        prev.success = true;
        vars.set('inspected', 'yes');
    "#,
    )];
    let mut user = UserContext::new("g", 0);
    let result = HttpSampler::new(config)
        .unwrap()
        .sample(&mut user)
        .await
        .unwrap();
    assert!(!result.success);
    assert!(!result.assertions[0].success);
    assert_eq!(result.response_code.as_deref(), Some("200"));
    assert_eq!(user.variables["inspected"], "yes");
    server.await.unwrap();
}

#[tokio::test]
async fn processor_failure_rolls_back_its_writes_keeps_prior_commits_and_stops_chain() {
    let (url, server) = http_response(response(b"hello")).await;
    let mut config = HttpRequestConfig::with_url("parent", url);
    config.post_processors = vec![
        processor("first", "vars.set('kept','yes');"),
        processor(
            "broken",
            "vars.remove('old'); vars.set('partial','bad'); throw new Error('extract failed');",
        ),
        processor("skip", "vars.set('ran', 'bad');"),
    ];
    let mut user = UserContext::new("g", 0);
    user.variables.insert("old".into(), "value".into());
    let result = HttpSampler::new(config)
        .unwrap()
        .sample(&mut user)
        .await
        .unwrap();
    assert!(!result.success);
    assert_eq!(result.label, "parent");
    assert_eq!(result.response_code.as_deref(), Some("200"));
    let message = result.response_message.unwrap();
    assert!(message.contains("OK") && message.contains("broken"));
    assert_eq!(user.variables["kept"], "yes");
    assert_eq!(user.variables["old"], "value");
    assert!(!user.variables.contains_key("partial"));
    assert!(!user.variables.contains_key("ran"));
    server.await.unwrap();
}

#[tokio::test]
async fn unavailable_bodies_are_null_with_reasons_and_responses_still_drain() {
    for (wire, limit, reason, expected_success) in [
        (response(b"abcdef"), 3, "exceeds", true),
        (response(&[0xff, 0xfe]), 10, "UTF-8", true),
        (
            b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\nx".to_vec(),
            10,
            "incomplete",
            false,
        ),
    ] {
        let (url, server) = http_response(wire).await;
        let mut config = HttpRequestConfig::with_url("unavailable", url);
        config.max_post_processor_body_bytes = limit;
        config.post_processors = vec![processor(
            "check",
            &format!(
                r#"
            if (prev.body !== null || !prev.bodyUnavailableReason.includes('{reason}')) throw new Error('unavailable input');
            vars.set('checked','yes');
        "#
            ),
        )];
        let mut user = UserContext::new("g", 0);
        let result = HttpSampler::new(config)
            .unwrap()
            .sample(&mut user)
            .await
            .unwrap();
        assert_eq!(
            result.success, expected_success,
            "{:?}",
            result.response_message
        );
        assert_eq!(user.variables["checked"], "yes");
        if reason == "exceeds" {
            assert_eq!(result.bytes_received, 6);
        }
        server.await.unwrap();
    }
}

#[tokio::test]
async fn http_failure_before_response_still_runs_processors_with_no_stale_body() {
    let mut config = HttpRequestConfig::with_url("invalid", "${missing}");
    config.post_processors = vec![processor(
        "failure handler",
        r#"
        if (prev.body !== null || prev.success || prev.responseCode !== null
            || !prev.bodyUnavailableReason) throw new Error('stale result');
        vars.set('handled', 'yes');
    "#,
    )];
    let mut user = UserContext::new("g", 0);
    let result = HttpSampler::new(config)
        .unwrap()
        .sample(&mut user)
        .await
        .unwrap();
    assert!(!result.success);
    assert_eq!(user.variables["handled"], "yes");
}

#[tokio::test]
async fn post_processor_timeout_rolls_back_and_keeps_http_result() {
    let (url, server) = http_response(response(b"ok")).await;
    let mut config = HttpRequestConfig::with_url("timeout", url);
    let mut script = processor("endless", "vars.set('partial','bad'); while(true) {}");
    script.timeout = Duration::from_millis(30);
    config.post_processors.push(script);
    let mut user = UserContext::new("g", 0);
    let result = HttpSampler::new(config)
        .unwrap()
        .sample(&mut user)
        .await
        .unwrap();
    assert!(!result.success);
    assert_eq!(result.response_code.as_deref(), Some("200"));
    assert!(result.response_message.unwrap().contains("Timeout"));
    assert!(!user.variables.contains_key("partial"));
    server.await.unwrap();
}

#[tokio::test]
async fn jdbc_failure_runs_post_processors_after_clearing_stale_result_variables() {
    let connection = Arc::new(
        JdbcConnection::new(JdbcConnectionConfig::new("db", "postgresql://localhost/db")).unwrap(),
    );
    let mut config = JdbcSamplerConfig::new("SQL", "db", "${missing}");
    config.result_variable = Some("result".into());
    config.save_response_body = false;
    config.post_processors = vec![processor(
        "failure handler",
        r#"
        if (prev.success || prev.body !== null || prev.responseCode !== 'INVALID_CONFIG'
            || !prev.bodyUnavailableReason || vars.contains('result')) throw new Error('stale JDBC result');
        vars.set('handled', 'yes');
    "#,
    )];
    let mut user = UserContext::new("g", 0);
    user.variables.insert("result".into(), "old".into());
    let result = JdbcSampler::new(config, connection)
        .unwrap()
        .sample(&mut user)
        .await
        .unwrap();
    assert!(!result.success);
    assert_eq!(user.variables["handled"], "yes");
}

#[tokio::test]
#[ignore = "requires RMETER_POSTGRES_URL"]
async fn jdbc_complete_result_is_available_without_report_body_storage() {
    let url = std::env::var("RMETER_POSTGRES_URL").expect("set RMETER_POSTGRES_URL");
    let connection = Arc::new(JdbcConnection::new(JdbcConnectionConfig::new("db", url)).unwrap());
    let mut config = JdbcSamplerConfig::new("SQL", "db", "SELECT $1::integer AS id");
    config.parameters = vec![json!("${id}")];
    config.pre_processors = vec![processor(
        "prepare id",
        "vars.put('id','42'); log.info('prepared {}', vars.get('id'));",
    )];
    config.save_response_body = false;
    config.post_processors = vec![processor(
        "extract id",
        r#"
        if (!prev.success || prev.responseCode !== '00000') throw new Error('query failed');
        const data = JSON.parse(prev.body);
        vars.set('userId', String(data.rows[0][data.columns.indexOf('id')]));
    "#,
    )];
    let mut user = UserContext::new("g", 0);
    let result = JdbcSampler::new(config, connection)
        .unwrap()
        .sample(&mut user)
        .await
        .unwrap();
    assert!(result.success, "{:?}", result.response_message);
    assert_eq!(user.variables["userId"], "42");
    assert!(result.response_body.is_none());
}

#[tokio::test]
async fn pre_processors_supply_http_variables_and_logs_survive_script_errors() {
    let (url, server) = http_response(response(b"ok")).await;
    let mut config = HttpRequestConfig::with_url("request", "${target}");
    config.pre_processors = vec![
        processor(
            "prepare",
            &format!(
                "vars.put('target', {}); log.info('user {{}}', ctx.getThreadNum());",
                json!(url)
            ),
        ),
        processor(
            "check",
            "if (ctx.getVariables().get('target') === null || vars.getIteration() !== 1 || sampler.getName() !== 'request' || Label !== 'check') throw new Error('bindings'); log.debug('ready');",
        ),
    ];
    config.post_processors = vec![processor(
        "extract",
        "if (prev.getResponseDataAsString() !== 'ok' || !prev.isSuccessful() || prev.getResponseCode() !== '200') throw new Error('prev'); log.warn('received {}', prev.getSampleLabel());",
    )];
    let mut user = UserContext::new("g", 2);
    let result = HttpSampler::new(config)
        .unwrap()
        .sample(&mut user)
        .await
        .unwrap();
    assert!(result.success, "{:?}", result.response_message);
    assert_eq!(
        user.script_logs
            .entries
            .iter()
            .map(|l| l.message.as_str())
            .collect::<Vec<_>>(),
        ["user 2", "ready", "received request"]
    );
    server.await.unwrap();

    let mut config = HttpRequestConfig::with_url("never sent", "http://127.0.0.1:1/");
    config.pre_processors = vec![
        processor(
            "broken",
            "log.error('before exception'); vars.put('partial', 'bad'); throw new Error('prepare failed');",
        ),
        processor("skip", "vars.put('ran','bad');"),
    ];
    config.post_processors = vec![processor("skip post", "vars.put('post','bad');")];
    let mut user = UserContext::new("g", 0);
    let result = HttpSampler::new(config)
        .unwrap()
        .sample(&mut user)
        .await
        .unwrap();
    assert!(!result.success);
    assert_eq!(result.bytes_sent, 0);
    assert!(result.response_message.unwrap().contains("broken"));
    assert!(user.variables.is_empty());
    assert_eq!(user.script_logs.entries[0].message, "before exception");
}

#[tokio::test]
async fn javascript_pre_main_post_share_variables_and_bound_logs() {
    let mut config = processor(
        "JS",
        "if (vars.get('phase') !== 'pre') throw new Error('order'); vars.put('phase','sample'); log.info('sample');",
    );
    config.pre_processors.push(processor(
        "pre",
        "vars.put('phase','pre'); log.info('pre');",
    ));
    config.post_processors.push(processor("post", "if (vars.get('phase') !== 'sample' || !prev.isSuccessful() || prev.body !== null || prev.label !== 'JS') throw new Error('order'); vars.put('phase','post'); log.info('post'); for(let i=0;i<200;i++) log.debug('x'.repeat(4096));"));
    let mut user = UserContext::new("g", 0);
    let result = JavaScriptSampler::new(config)
        .unwrap()
        .sample(&mut user)
        .await
        .unwrap();
    assert!(result.success, "{:?}", result.response_message);
    assert_eq!(user.variables["phase"], "post");
    assert!(user.script_logs.truncated);
    assert!(user.script_logs.entries.len() <= 100);
    assert!(
        user.script_logs
            .entries
            .iter()
            .map(|l| l.message.len() + l.source.len() + l.level.len())
            .sum::<usize>()
            <= 65536
    );
    assert_eq!(
        user.script_logs.entries[..3]
            .iter()
            .map(|l| l.message.as_str())
            .collect::<Vec<_>>(),
        ["pre", "sample", "post"]
    );
}

#[tokio::test]
async fn pre_processor_timeout_skips_parent_and_rolls_back_variables() {
    let mut config = processor("skip main", "vars.put('main','bad');");
    let mut pre = processor("timeout pre", "vars.put('partial','bad'); while(true){}");
    pre.timeout = Duration::from_millis(30);
    config.pre_processors.push(pre);
    config
        .post_processors
        .push(processor("skip post", "vars.put('post','bad');"));
    let mut user = UserContext::new("g", 0);
    let result = JavaScriptSampler::new(config)
        .unwrap()
        .sample(&mut user)
        .await
        .unwrap();
    assert!(!result.success);
    assert_eq!(result.response_code.as_deref(), Some("Timeout"));
    assert!(user.variables.is_empty());
}
