//! Regression coverage for TLS downgrade, parser crashes and non-yielding Boa operations.
use rmeter_component::{
    JavaScriptConfig, JavaScriptSampler, JdbcConnection, JdbcConnectionConfig, JdbcSampler,
    JdbcSamplerConfig, javascript::set_worker_executable,
};
use rmeter_core::{context::UserContext, sampler::Sampler};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

fn config(script: impl Into<String>) -> JavaScriptConfig {
    set_worker_executable(env!("CARGO_BIN_EXE_rmeter-js-worker")).unwrap();
    let mut config = JavaScriptConfig::new("security regression", script);
    config.timeout = Duration::from_millis(200);
    config
}

#[test]
fn deeply_nested_source_cannot_crash_the_host_during_validation() {
    let source = format!("let x = {}1{};", "(".repeat(10_000), ")".repeat(10_000));
    let started = Instant::now();
    let error = JavaScriptSampler::new(config(source)).unwrap_err();
    assert!(
        error.to_string().contains("worker") || error.to_string().contains("deadline"),
        "{error}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    JavaScriptSampler::new(config("const normal = 1;")).unwrap();
}

#[tokio::test]
async fn native_regex_is_killed_at_the_parent_deadline_and_writes_are_rolled_back() {
    let sampler = JavaScriptSampler::new(config(
        "vars.set('partial','bad'); /^(a+)+$/.test('a'.repeat(30) + '!');",
    ))
    .unwrap();
    let mut user = UserContext::new("g", 0);
    let started = Instant::now();
    let result = tokio::time::timeout(Duration::from_secs(5), sampler.sample(&mut user))
        .await
        .unwrap()
        .unwrap();
    assert!(!result.success);
    assert_eq!(result.response_code.as_deref(), Some("Timeout"));
    assert!(started.elapsed() < Duration::from_secs(3));
    assert!(user.variables.is_empty());
    let normal = JavaScriptSampler::new(config("vars.set('alive','yes');")).unwrap();
    assert!(normal.sample(&mut user).await.unwrap().success);
    assert_eq!(user.variables["alive"], "yes");
}

#[tokio::test]
async fn oversized_allocation_is_confined_to_the_worker() {
    let mut cfg = config("vars.set('partial','bad'); new Uint8Array(300 * 1024 * 1024).fill(1);");
    cfg.timeout = Duration::from_secs(2);
    let sampler = JavaScriptSampler::new(cfg).unwrap();
    let mut user = UserContext::new("g", 0);
    let result = tokio::time::timeout(Duration::from_secs(5), sampler.sample(&mut user))
        .await
        .unwrap()
        .unwrap();
    assert!(!result.success);
    assert!(user.variables.is_empty());
    assert!(
        JavaScriptSampler::new(config("1 + 1;"))
            .unwrap()
            .sample(&mut user)
            .await
            .unwrap()
            .success
    );
}

#[tokio::test]
async fn dropping_non_yielding_samples_releases_worker_capacity() {
    for _ in 0..4 {
        let mut cfg = config("/^(a+)+$/.test('a'.repeat(30) + '!');");
        cfg.timeout = Duration::from_secs(60);
        let sampler = JavaScriptSampler::new(cfg).unwrap();
        let task = tokio::spawn(async move { sampler.sample(&mut UserContext::new("g", 0)).await });
        tokio::time::sleep(Duration::from_millis(80)).await;
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
    }
    // If cancelled workers were left running, the four-slot queue would be full.
    let started = Instant::now();
    let sampler = JavaScriptSampler::new(config("vars.set('alive','yes');")).unwrap();
    assert!(
        sampler
            .sample(&mut UserContext::new("g", 0))
            .await
            .unwrap()
            .success
    );
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn jdbc_refuses_tls_downgrade_before_sending_startup_or_password() {
    for suffix in ["", "?sslmode=prefer", "?sslmode=require"] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "postgresql://audit@127.0.0.1:{}/audit{suffix}",
            listener.local_addr().unwrap().port()
        );
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut ssl_request = [0; 8];
            stream.read_exact(&mut ssl_request).await.unwrap();
            assert_eq!(ssl_request, [0, 0, 0, 8, 4, 210, 22, 47]);
            stream.write_all(b"N").await.unwrap();
            let mut next = [0; 1];
            assert_eq!(
                stream.read(&mut next).await.unwrap(),
                0,
                "client must close without sending startup or password"
            );
        });
        let mut config = JdbcConnectionConfig::new("db", url);
        config.password = Some("audit-only-dummy-password".into());
        let connection = Arc::new(JdbcConnection::new(config).unwrap());
        let sampler = JdbcSampler::new(
            JdbcSamplerConfig::new("probe", "db", "SELECT 1"),
            connection,
        )
        .unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            sampler.sample(&mut UserContext::new("g", 0)),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!result.success);
        assert!(result.response_message.as_deref().unwrap().contains("TLS"));
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }
}

#[tokio::test]
async fn explicit_disable_remains_an_opt_in_for_local_plaintext_databases() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "postgresql://audit@127.0.0.1:{}/audit?sslmode=disable",
        listener.local_addr().unwrap().port()
    );
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let n = stream.read_u32().await.unwrap();
        let mut startup = vec![0; n as usize - 4];
        stream.read_exact(&mut startup).await.unwrap();
        assert_eq!(&startup[..4], &[0, 3, 0, 0]);
        stream
            .write_all(&[b'R', 0, 0, 0, 8, 0, 0, 0, 3])
            .await
            .unwrap();
        assert_eq!(stream.read_u8().await.unwrap(), b'p');
        let n = stream.read_u32().await.unwrap();
        let mut password = vec![0; n as usize - 4];
        stream.read_exact(&mut password).await.unwrap();
        assert_eq!(password, b"audit-only-dummy-password\0");
    });
    let mut config = JdbcConnectionConfig::new("db", url);
    config.password = Some("audit-only-dummy-password".into());
    let connection = Arc::new(JdbcConnection::new(config).unwrap());
    let sampler = JdbcSampler::new(
        JdbcSamplerConfig::new("probe", "db", "SELECT 1"),
        connection,
    )
    .unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        sampler.sample(&mut UserContext::new("g", 0)),
    )
    .await
    .unwrap()
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}
