use rmeter_component::{
    ComponentCompiler, ComponentNode, JdbcConnection, JdbcConnectionConfig, JdbcQueryType,
    JdbcSampler, JdbcSamplerConfig, JsonAssertionConfig,
};
use rmeter_core::{
    context::UserContext,
    plan::Node,
    sampler::{SampleResult, Sampler},
};
use serde_json::{Value, json};
use std::{collections::HashMap, sync::Arc, time::Duration};

fn connection(url: &str) -> Arc<JdbcConnection> {
    Arc::new(JdbcConnection::new(JdbcConnectionConfig::new("db", url)).unwrap())
}
fn body(result: &SampleResult) -> Value {
    serde_json::from_slice(result.response_body.as_ref().unwrap()).unwrap()
}
async fn sample(connection: Arc<JdbcConnection>, sql: &str, kind: JdbcQueryType) -> SampleResult {
    let mut config = JdbcSamplerConfig::new("test SQL", "db", sql);
    config.query_type = kind;
    JdbcSampler::new(config, connection)
        .unwrap()
        .sample(&mut UserContext::new("g", 0))
        .await
        .unwrap()
}
fn real_connection() -> Arc<JdbcConnection> {
    connection(
        &std::env::var("RMETER_POSTGRES_URL")
            .expect("set RMETER_POSTGRES_URL to an isolated PostgreSQL test database"),
    )
}

#[test]
fn validates_connection_and_sampler_settings_without_connecting() {
    for url in [
        "jdbc:mysql://localhost/db",
        "not a URL",
        "postgresql://[invalid",
    ] {
        assert!(JdbcConnection::new(JdbcConnectionConfig::new("db", url)).is_err());
    }
    let mut config = JdbcConnectionConfig::new("db", "postgresql://localhost/db");
    config.max_connections = 0;
    assert!(JdbcConnection::new(config).is_err());
    let mut config = JdbcConnectionConfig::new("db", "postgresql://user:secret@localhost/db");
    config.password = Some("secret".into());
    config.password_env = Some("PGPASSWORD".into());
    assert!(!format!("{config:?}").contains("secret"));
    assert!(JdbcConnection::new(config).is_err());
    let connection = connection("jdbc:postgresql://localhost/db");
    let mut config = JdbcSamplerConfig::new("query", "db", "SELECT 1");
    config.query_timeout = Duration::ZERO;
    assert!(JdbcSampler::new(config, connection.clone()).is_err());
    let mut config = JdbcSamplerConfig::new("query", "wrong", "SELECT 1");
    assert!(JdbcSampler::new(config.clone(), connection.clone()).is_err());
    config.connection = "db".into();
    config.max_rows = 0;
    assert!(JdbcSampler::new(config, connection).is_err());
}

#[test]
fn named_connections_have_lexical_scope_and_configuration_nodes_emit_no_samples() {
    let outer = JdbcConnectionConfig::new("db", "postgresql://localhost/outer");
    let inner = JdbcConnectionConfig::new("db", "postgresql://localhost/inner");
    let request = || ComponentNode::JdbcRequest(JdbcSamplerConfig::new("query", "db", "SELECT 1"));
    let mut compiler = ComponentCompiler::new();
    let nodes = compiler
        .compile(
            vec![
                request(),
                ComponentNode::Sequence(vec![request(), ComponentNode::JdbcConnection(inner)]),
                request(),
                ComponentNode::JdbcConnection(outer),
            ],
            &Default::default(),
        )
        .unwrap();
    assert_eq!(nodes.len(), 3);
    let Node::Sample(first) = &nodes[0] else {
        panic!()
    };
    let Node::Sequence(inner) = &nodes[1] else {
        panic!()
    };
    let Node::Sample(inside) = &inner[0] else {
        panic!()
    };
    let Node::Sample(last) = &nodes[2] else {
        panic!()
    };
    let first = first.as_jdbc().unwrap().connection();
    assert!(Arc::ptr_eq(first, last.as_jdbc().unwrap().connection()));
    assert!(!Arc::ptr_eq(first, inside.as_jdbc().unwrap().connection()));
    assert!(
        inside
            .as_jdbc()
            .unwrap()
            .connection()
            .config()
            .url
            .ends_with("/inner")
    );
    assert!(
        compiler
            .compile(vec![request()], &Default::default())
            .is_err()
    );
    let duplicate = JdbcConnectionConfig::new("db", "postgresql://localhost/db");
    assert!(
        compiler
            .compile(
                vec![
                    ComponentNode::JdbcConnection(duplicate.clone()),
                    ComponentNode::JdbcConnection(duplicate)
                ],
                &Default::default()
            )
            .is_err()
    );
}

#[tokio::test]
async fn missing_runtime_variables_fail_with_diagnostics_and_clear_stale_results() {
    let mut config = JdbcSamplerConfig::new("missing", "db", "SELECT $1::int");
    config.parameters = vec![json!("${missing}")];
    config.result_variable = Some("result".into());
    config.row_count_variable = Some("count".into());
    config.variable_names = vec!["id".into()];
    config.add_assertion(JsonAssertionConfig::equals("row", "$.row_count", json!(1)));
    let sampler = JdbcSampler::new(config, connection("postgresql://localhost/db")).unwrap();
    let mut context = UserContext::new("g", 0);
    context.variables = HashMap::from([
        ("result".into(), "old".into()),
        ("count".into(), "8".into()),
        ("result_#".into(), "8".into()),
        ("id_#".into(), "2".into()),
        ("id_1".into(), "old".into()),
        ("id_2".into(), "old".into()),
    ]);
    let result = sampler.sample(&mut context).await.unwrap();
    assert!(!result.success);
    assert_eq!(result.response_code.as_deref(), Some("INVALID_CONFIG"));
    assert_eq!(result.assertions.len(), 1);
    assert!(!result.assertions[0].success);
    assert!(!context.variables.contains_key("result"));
    assert!(!context.variables.contains_key("count"));
    assert!(!context.variables.contains_key("result_#"));
    assert!(!context.variables.contains_key("id_1"));
    assert!(!context.variables.contains_key("id_2"));
    assert!(!context.variables.contains_key("id_#"));
}

#[tokio::test]
#[ignore = "requires RMETER_POSTGRES_URL; read-only generated rows"]
async fn postgres_result_arrays_and_column_variables_replace_previous_rows() {
    let connection = real_connection();
    let mut config = JdbcSamplerConfig::new(
        "variables",
        "db",
        "SELECT n AS id, 'skip'::text AS ignored, CASE WHEN n=2 THEN NULL ELSE '用户-' || n END AS name, n%2=0 AS enabled, jsonb_build_object('n',n) AS payload FROM generate_series(1,$1::int) n ORDER BY n",
    );
    config.parameters = vec![json!("${limit}")];
    config.result_variable = Some("rows".into());
    config.variable_names = vec![
        "id".into(),
        "".into(),
        "name".into(),
        "enabled".into(),
        "payload".into(),
        "extra".into(),
    ];
    config.save_response_body = false;
    let sampler = JdbcSampler::new(config.clone(), connection.clone()).unwrap();
    let mut context = UserContext::new("g", 0);
    for count in [3, 1, 0, 2] {
        context.variables.insert("limit".into(), count.to_string());
        let sample = sampler.sample(&mut context).await.unwrap();
        assert!(sample.success, "{:?}", sample.response_message);
        assert!(sample.response_body.is_none());
        let rows: Value = serde_json::from_str(&context.variables["rows"]).unwrap();
        assert_eq!(rows.as_array().unwrap().len(), count);
        for name in ["rows", "id", "name", "enabled", "payload", "extra"] {
            assert_eq!(context.variables[&format!("{name}_#")], count.to_string());
        }
        assert!(!context.variables.contains_key(&format!("id_{}", count + 1)));
        assert!(!context.variables.contains_key("name_2"));
        assert!(!context.variables.contains_key("extra_1"));
        if count > 0 {
            assert_eq!(rows[0]["id"], 1);
            assert_eq!(context.variables["id_1"], "1");
            assert_eq!(context.variables["name_1"], "用户-1");
            assert_eq!(context.variables["enabled_1"], "false");
            assert_eq!(
                serde_json::from_str::<Value>(&context.variables["payload_1"]).unwrap(),
                json!({"n":1})
            );
        }
    }
    let mut chained = config.clone();
    chained.parameters = vec![json!("${id_#}")];
    assert!(
        JdbcSampler::new(chained, connection.clone())
            .unwrap()
            .sample(&mut context)
            .await
            .unwrap()
            .success
    );
    assert_eq!(context.variables["id_#"], "2");
    let mut isolated = UserContext::new("g", 1);
    isolated.variables.insert("limit".into(), "1".into());
    assert!(sampler.sample(&mut isolated).await.unwrap().success);
    assert_eq!(context.variables["id_#"], "2");
    assert_eq!(isolated.variables["id_#"], "1");
    for sql in ["SELECT 1/0", "SELECT pg_sleep(1)"] {
        let mut failed = config.clone();
        failed.sql = sql.into();
        failed.parameters.clear();
        failed.query_timeout = Duration::from_millis(100);
        let failure = JdbcSampler::new(failed, connection.clone())
            .unwrap()
            .sample(&mut context)
            .await
            .unwrap();
        assert!(!failure.success);
        for key in ["rows", "rows_#", "id_#", "id_1", "id_2"] {
            assert!(!context.variables.contains_key(key));
        }
        assert!(sampler.sample(&mut context).await.unwrap().success);
    }
}

#[tokio::test]
#[ignore = "requires RMETER_POSTGRES_URL"]
async fn postgres_parameters_results_types_pool_reuse_and_assertions() {
    let connection = real_connection();
    let mut config = JdbcSamplerConfig::new(
        "typed",
        "db",
        "SELECT $1::integer, $2::bigint, $3::text, $4::boolean, $5::jsonb, $6::text, 12345678901234567890.123400::numeric, DATE '2026-10-20', '00000000-0000-0000-0000-000000000001'::uuid, ARRAY[1,NULL,3], decode('00ff','hex'), TIMESTAMP '2026-10-20 12:30:00', TIMESTAMPTZ '2026-10-20 12:30:00+00', 1.25::float8",
    );
    config.parameters = vec![
        json!(42),
        json!(9223372036854775807i64),
        json!("${name}"),
        json!(true),
        json!({"a":1}),
        Value::Null,
    ];
    config.result_variable = Some("result".into());
    config.row_count_variable = Some("count".into());
    config.add_assertion(JsonAssertionConfig::equals(
        "integer",
        "$.rows[0][0]",
        json!(42),
    ));
    let sampler = JdbcSampler::new(config, connection.clone()).unwrap();
    let mut context = UserContext::new("g", 0);
    context
        .variables
        .insert("name".into(), "Robert'); DROP TABLE never;-- 中文".into());
    for _ in 0..2 {
        let result = sampler.sample(&mut context).await.unwrap();
        assert!(result.success, "{:?}", result.response_message);
        assert_eq!(result.response_code.as_deref(), Some("00000"));
        let output = body(&result);
        assert_eq!(output["rows"][0][1], json!(9223372036854775807i64));
        assert_eq!(output["rows"][0][2], context.variables["name"]);
        assert_eq!(output["rows"][0][3], true);
        assert_eq!(output["rows"][0][4], json!({"a":1}));
        assert_eq!(output["rows"][0][5], Value::Null);
        assert_eq!(output["rows"][0][6], "12345678901234567890.123400");
        assert_eq!(output["rows"][0][7], "2026-10-20");
        assert_eq!(output["rows"][0][8], "00000000-0000-0000-0000-000000000001");
        assert_eq!(output["rows"][0][9], json!([1, null, 3]));
        assert_eq!(output["rows"][0][10], "\\x00ff");
        assert_eq!(output["rows"][0][11], "2026-10-20T12:30:00");
        assert_eq!(output["rows"][0][12], "2026-10-20T12:30:00+00:00");
        assert_eq!(output["rows"][0][13], 1.25);
        assert_eq!(context.variables["count"], "1");
        let expected: serde_json::Map<String, Value> = output["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_str().unwrap().to_owned())
            .zip(output["rows"][0].as_array().unwrap().iter().cloned())
            .collect();
        assert_eq!(
            serde_json::from_str::<Value>(&context.variables["result"]).unwrap(),
            json!([expected])
        );
        assert_eq!(context.variables["result_#"], "1");
    }
    let sql = "SELECT pg_backend_pid()";
    let a = sample(connection.clone(), sql, JdbcQueryType::Select).await;
    let b = sample(connection.clone(), sql, JdbcQueryType::Select).await;
    assert_eq!(body(&a)["rows"][0][0], body(&b)["rows"][0][0]);
}

#[tokio::test]
#[ignore = "requires RMETER_POSTGRES_URL"]
async fn postgres_update_error_limits_and_parameter_count_rollback() {
    let connection = real_connection();
    assert!(
        sample(
            connection.clone(),
            "CREATE TABLE IF NOT EXISTS rmeter_sampler_test (id integer primary key, value text)",
            JdbcQueryType::Update
        )
        .await
        .success
    );
    let update = sample(connection.clone(), "INSERT INTO rmeter_sampler_test VALUES (1, 'one') ON CONFLICT(id) DO UPDATE SET value='one'", JdbcQueryType::Update).await;
    assert_eq!(body(&update)["rows_affected"], 1);
    let mut config = JdbcSamplerConfig::new(
        "limit",
        "db",
        "INSERT INTO rmeter_sampler_test SELECT n,'value' FROM generate_series(10,12) n RETURNING id",
    );
    config.max_rows = 1;
    let result = JdbcSampler::new(config, connection.clone())
        .unwrap()
        .sample(&mut UserContext::new("g", 0))
        .await
        .unwrap();
    assert_eq!(result.response_code.as_deref(), Some("RESULT_LIMIT"));
    let result = sample(
        connection.clone(),
        "SELECT count(*) FROM rmeter_sampler_test WHERE id>=10",
        JdbcQueryType::Select,
    )
    .await;
    assert_eq!(body(&result)["rows"][0][0], 0);
    let result = sample(
        connection.clone(),
        "SELECT * FROM table_that_does_not_exist_rmeter",
        JdbcQueryType::Select,
    )
    .await;
    assert!(!result.success);
    assert_eq!(result.response_code.as_deref(), Some("42P01"));
    let result = sample(
        connection.clone(),
        "SELECT $1::integer",
        JdbcQueryType::Select,
    )
    .await;
    assert_eq!(result.response_code.as_deref(), Some("INVALID_CONFIG"));
    let mut config = JdbcSamplerConfig::new("bytes", "db", "SELECT repeat('x',200)");
    config.max_result_bytes = 100;
    let result = JdbcSampler::new(config, connection.clone())
        .unwrap()
        .sample(&mut UserContext::new("g", 0))
        .await
        .unwrap();
    assert_eq!(result.response_code.as_deref(), Some("RESULT_LIMIT"));
    assert!(
        sample(
            connection,
            "DROP TABLE rmeter_sampler_test",
            JdbcQueryType::Update
        )
        .await
        .success
    );
}

#[tokio::test]
#[ignore = "requires RMETER_POSTGRES_URL"]
async fn postgres_timeout_and_aborted_future_cancel_the_backend_and_allow_reuse() {
    let connection = real_connection();
    assert!(
        sample(connection.clone(), "SELECT 1", JdbcQueryType::Select)
            .await
            .success
    );
    let mut config = JdbcSamplerConfig::new("timeout", "db", "SELECT pg_sleep(10)");
    config.query_timeout = Duration::from_millis(150);
    let sampler = JdbcSampler::new(config, connection.clone()).unwrap();
    let result = sampler.sample(&mut UserContext::new("g", 0)).await.unwrap();
    assert!(!result.success);
    assert!(matches!(
        result.response_code.as_deref(),
        Some("TIMEOUT" | "57014")
    ));
    assert!(result.elapsed < Duration::from_secs(2));
    assert!(
        sample(connection.clone(), "SELECT 1", JdbcQueryType::Select)
            .await
            .success
    );

    let observer = real_connection();
    let sampler = JdbcSampler::new(
        JdbcSamplerConfig::new("cancel", "db", "SELECT pg_sleep(10)"),
        connection.clone(),
    )
    .unwrap();
    let task = tokio::spawn(async move { sampler.sample(&mut UserContext::new("g", 0)).await });
    let active = "SELECT count(*) FROM pg_stat_activity WHERE application_name='rmeter' AND state='active' AND query='SELECT pg_sleep(10)'";
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let result = sample(observer.clone(), active, JdbcQueryType::Select).await;
            if body(&result)["rows"][0][0].as_i64().unwrap() > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let result = sample(observer.clone(), active, JdbcQueryType::Select).await;
            if body(&result)["rows"][0][0] == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        sample(connection, "SELECT 1", JdbcQueryType::Select)
            .await
            .success
    );
}
