#![cfg(feature = "cli")]

mod support;

use serde_json::{Value, json};
use std::{
    ffi::OsString,
    process::{Command, Output},
    time::Duration,
};
use support::{TempScript, script, server};

async fn invoke(arguments: Vec<OsString>) -> Output {
    tokio::time::timeout(
        Duration::from_secs(10),
        tokio::task::spawn_blocking(move || {
            Command::new(env!("CARGO_BIN_EXE_rmeter"))
                .args(arguments)
                .output()
                .unwrap()
        }),
    )
    .await
    .expect("CLI did not finish")
    .unwrap()
}
fn events(output: &Output) -> Vec<Value> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[tokio::test]
async fn javascript_check_fails_closed_if_the_worker_is_missing() {
    let file = TempScript::new(
        &json!({"version":1,"name":"missing worker","groups":[{
            "name":"g","children":[{"type":"javascript","name":"JS","script":"1 + 1;"}]
        }]})
        .to_string(),
    );
    let missing = file.0.with_extension(std::env::consts::EXE_EXTENSION);
    let output = tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_rmeter"))
            .arg(&file.0)
            .arg("--check")
            .env("RMETER_JS_WORKER", missing)
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("isolation failed"));
    assert!(output.stdout.is_empty());
}

#[tokio::test]
async fn cli_runs_file_streams_json_and_sets_success_or_failure_exit_code() {
    for (body, code) in [(r#"{"code":0}"#, 0), (r#"{"code":7}"#, 1)] {
        let (url, task) = server(body, 1).await;
        let file = TempScript::new(&script(&url).to_string());
        let output = invoke(vec![file.0.as_os_str().to_owned()]).await;
        task.await.unwrap();
        assert_eq!(
            output.status.code(),
            Some(code),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let reports = events(&output);
        assert_eq!(reports.len(), 4);
        assert_eq!(reports[0]["type"], "started");
        assert_eq!(reports[1]["type"], "sample_completed");
        assert_eq!(reports[1]["success"], code == 0);
        assert_eq!(reports[2]["type"], "finished");
        assert_eq!(reports[3]["type"], "summary");
        assert_eq!(reports[3]["failed"], code);
        assert_eq!(reports[3]["error"], Value::Null);
        assert_eq!(reports[3]["cancelled"], false);
    }
}

#[tokio::test]
async fn check_only_does_not_send_requests() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let file = TempScript::new(
        &script(&format!("http://{}/", listener.local_addr().unwrap())).to_string(),
    );
    let output = invoke(vec![file.0.as_os_str().to_owned(), "--check".into()]).await;
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(events(&output)[0]["type"], "validated");
    assert!(
        tokio::time::timeout(Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn variables_override_in_order_and_keep_equals_in_values() {
    let (url, task) = server(r#"{"code":0}"#, 1).await;
    let mut source = script("${url}");
    source["variables"] = json!({"url":"http://bad.invalid/", "token":"bad"});
    source["groups"][0]["children"][0]["settings"] = json!({"headers":[["X-Token","${token}"]]});
    let file = TempScript::new(&source.to_string());
    let output = invoke(vec![
        file.0.as_os_str().to_owned(),
        "--var".into(),
        format!("url={url}").into(),
        "--var".into(),
        "token=first".into(),
        "--var".into(),
        "token=a=b".into(),
    ])
    .await;
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(task.await.unwrap()[0].contains("x-token: a=b\r\n"));
}

#[tokio::test]
async fn cli_reports_argument_file_and_static_configuration_errors() {
    let file = TempScript::new("{ invalid JSON }");
    let good = TempScript::new(&script("http://localhost/").to_string());
    let mut invalid = script("http://localhost/");
    invalid["groups"][0]["children"][0]["assertions"][0]["path"] = json!("$[");
    let invalid = TempScript::new(&invalid.to_string());
    for arguments in [
        vec![],
        vec![file.0.as_os_str().to_owned()],
        vec![file.0.with_extension("missing").into_os_string()],
        vec![good.0.as_os_str().to_owned(), "--var".into(), "bad".into()],
        vec![invalid.0.as_os_str().to_owned(), "--check".into()],
    ] {
        let output = invoke(arguments).await;
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    }
}

#[tokio::test]
async fn controller_runtime_error_produces_error_summary_and_exit_two() {
    let source = json!({"version":1,"name":"error","groups":[{"name":"g","children":[{
        "type":"if","name":"missing condition","condition":"${missing}","children":[]
    }]}]});
    let file = TempScript::new(&source.to_string());
    let output = invoke(vec![file.0.as_os_str().to_owned()]).await;
    assert_eq!(output.status.code(), Some(2));
    let reports = events(&output);
    let summary = reports.last().unwrap();
    assert_eq!(summary["type"], "summary");
    assert_eq!(summary["samples"], 0);
    assert!(summary["error"].as_str().unwrap().contains("missing"));
}

#[tokio::test]
async fn help_and_version_work_without_a_script() {
    for argument in ["--help", "--version"] {
        let output = invoke(vec![argument.into()]).await;
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("rmeter"));
    }
}
