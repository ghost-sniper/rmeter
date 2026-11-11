use deadpool_postgres::Client;
use futures_util::TryStreamExt;
use postgres_native_tls::MakeTlsConnector;
use rmeter_core::{
    assertion::AssertionInput,
    context::UserContext,
    sampler::{SampleError, SampleResult, Sampler},
};
use serde_json::{Value, json};
use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};
use tokio_postgres::types::ToSql;

use super::{
    JdbcConfigError, JdbcConnection, JdbcQueryType, JdbcSamplerConfig,
    config::valid_duration,
    value::{Cell, Parameter},
};
use crate::{ResponseAssertion, assertion::evaluate_assertions, http::template};

#[derive(Debug, Clone)]
pub struct JdbcSampler {
    config: JdbcSamplerConfig,
    connection: Arc<JdbcConnection>,
    assertions: Vec<ResponseAssertion>,
    pre_processors: Vec<crate::JavaScriptSampler>,
    post_processors: Vec<crate::JavaScriptSampler>,
}
impl JdbcSampler {
    pub fn new(
        mut config: JdbcSamplerConfig,
        connection: Arc<JdbcConnection>,
    ) -> Result<Self, JdbcConfigError> {
        if config.label.trim().is_empty()
            || config.sql.trim().is_empty()
            || config.connection != connection.config().variable_name
        {
            return Err(JdbcConfigError::new(
                "JDBC sampler requires a name, SQL, and matching connection reference",
            ));
        }
        valid_duration(config.query_timeout, "query_timeout")?;
        if config.query_timeout.as_millis() > i32::MAX as u128 {
            return Err(JdbcConfigError::new(
                "query_timeout exceeds PostgreSQL statement_timeout range",
            ));
        }
        if config.max_rows == 0 || config.max_result_bytes == 0 {
            return Err(JdbcConfigError::new(
                "JDBC max_rows and max_result_bytes must be positive",
            ));
        }
        for key in [&config.result_variable, &config.row_count_variable]
            .into_iter()
            .flatten()
        {
            if key.trim().is_empty() {
                return Err(JdbcConfigError::new(
                    "JDBC result variable names must not be empty",
                ));
            }
        }
        if config.result_variable.is_some() && config.result_variable == config.row_count_variable {
            return Err(JdbcConfigError::new(
                "result_variable and row_count_variable must differ",
            ));
        }
        super::variables::validate(&mut config)?;
        for value in [&config.label, &config.sql] {
            template::validate(value).map_err(|e| JdbcConfigError::new(e.message))?;
        }
        // Validate parameter templates without needing variables at compile time.
        fn validate(value: &Value) -> Result<(), JdbcConfigError> {
            match value {
                Value::String(text) => {
                    template::validate(text).map_err(|e| JdbcConfigError::new(e.message))
                }
                Value::Array(values) => {
                    for value in values {
                        validate(value)?;
                    }
                    Ok(())
                }
                Value::Object(values) => {
                    for value in values.values() {
                        validate(value)?;
                    }
                    Ok(())
                }
                _ => Ok(()),
            }
        }
        for parameter in &config.parameters {
            validate(parameter)?;
        }
        let assertions = config
            .assertions
            .iter()
            .cloned()
            .map(ResponseAssertion::new)
            .collect::<Result<_, _>>()
            .map_err(|e| JdbcConfigError::new(e.to_string()))?;
        crate::timer::validate(&config.timers).map_err(JdbcConfigError::new)?;
        let pre_processors = crate::post_processor::compile(&config.pre_processors)
            .map_err(|e| JdbcConfigError::new(e.to_string()))?;
        let post_processors = crate::post_processor::compile(&config.post_processors)
            .map_err(|e| JdbcConfigError::new(e.to_string()))?;
        Ok(Self {
            config,
            connection,
            assertions,
            pre_processors,
            post_processors,
        })
    }
    pub fn config(&self) -> &JdbcSamplerConfig {
        &self.config
    }
    pub fn connection(&self) -> &Arc<JdbcConnection> {
        &self.connection
    }

    async fn execute(
        &self,
        parameters: &[Parameter],
        sql: &str,
        variables: &std::collections::HashMap<String, String>,
    ) -> Result<Payload, Failure> {
        let (pool, tls) = self
            .connection
            .pool(variables)
            .map_err(|e| Failure::config(e.message))?;
        let client = pool.get().await.map_err(|e| match e {
            deadpool_postgres::PoolError::Backend(e) => Failure::from(e),
            deadpool_postgres::PoolError::Timeout(_) => {
                Failure("TIMEOUT".into(), "JDBC connection pool timed out".into())
            }
            e => Failure("JDBC_POOL_ERROR".into(), e.to_string()),
        })?;
        let mut lease = Lease {
            client: Some(client),
            tls,
            clean: false,
        };
        let client = lease.client.as_mut().unwrap();
        let transaction = client.transaction().await?;
        let timeout_ms = self.config.query_timeout.as_millis().max(1).to_string();
        transaction
            .query_one(
                "SELECT set_config('statement_timeout', $1, true)",
                &[&timeout_ms],
            )
            .await?;
        let statement = transaction.prepare_cached(sql).await?;
        if statement.params().len() != parameters.len() {
            return Err(Failure::config(format!(
                "expected {} SQL parameters, got {}",
                statement.params().len(),
                parameters.len()
            )));
        }
        let bindings = parameters
            .iter()
            .map(|p| p as &(dyn ToSql + Sync))
            .collect::<Vec<_>>();
        let columns = statement
            .columns()
            .iter()
            .map(|c| c.name().to_owned())
            .collect::<Vec<_>>();
        let (rows, affected) = match self.config.query_type {
            JdbcQueryType::Update => (
                Vec::new(),
                transaction.execute(&statement, &bindings).await?,
            ),
            JdbcQueryType::Select => {
                let mut stream = Box::pin(transaction.query_raw(&statement, bindings).await?);
                let mut rows = Vec::new();
                let mut size = 0usize;
                while let Some(row) = stream.try_next().await? {
                    if rows.len() == self.config.max_rows {
                        return Err(Failure::limit("JDBC row limit exceeded"));
                    }
                    let values = (0..row.len())
                        .map(|index| row.try_get::<_, Cell>(index).map(|v| v.0))
                        .collect::<Result<Vec<_>, _>>()?;
                    size = size.saturating_add(
                        serde_json::to_vec(&values)
                            .map_err(|e| Failure::limit(e.to_string()))?
                            .len(),
                    );
                    if size > self.config.max_result_bytes {
                        return Err(Failure::limit("JDBC result byte limit exceeded"));
                    }
                    rows.push(values);
                }
                let affected = stream.rows_affected().unwrap_or(rows.len() as u64);
                (rows, affected)
            }
        };
        let count = rows.len();
        let variables =
            super::variables::export(&self.config, &columns, &rows).map_err(Failure::limit)?;
        let body = serde_json::to_vec(
            &json!({"columns":columns,"rows":rows,"row_count":count,"rows_affected":affected}),
        )
        .map_err(|e| Failure::limit(e.to_string()))?;
        if body.len() > self.config.max_result_bytes {
            return Err(Failure::limit("JDBC result byte limit exceeded"));
        }
        transaction.commit().await?;
        lease.clean = true;
        Ok(Payload { body, variables })
    }
}
struct Payload {
    body: Vec<u8>,
    variables: std::collections::HashMap<String, String>,
}
struct Failure(String, String);
impl Failure {
    fn config(message: impl Into<String>) -> Self {
        Self("INVALID_CONFIG".into(), message.into())
    }
    fn limit(message: impl Into<String>) -> Self {
        Self("RESULT_LIMIT".into(), message.into())
    }
}
impl From<tokio_postgres::Error> for Failure {
    fn from(error: tokio_postgres::Error) -> Self {
        match error.as_db_error() {
            Some(db) => Self(db.code().code().into(), db.message().into()),
            None => {
                let mut message = error.to_string();
                let mut source = std::error::Error::source(&error);
                for _ in 0..4 {
                    let Some(cause) = source else {
                        break;
                    };
                    message.push_str(&format!(": {cause}"));
                    source = cause.source();
                }
                Self("JDBC_ERROR".into(), message)
            }
        }
    }
}

impl Sampler for JdbcSampler {
    async fn sample(&self, context: &mut UserContext) -> Result<SampleResult, SampleError> {
        let started = Instant::now();
        let mut result = SampleResult {
            label: self.config.label.clone(),
            started_at: SystemTime::now(),
            elapsed: Duration::ZERO,
            success: false,
            response_code: None,
            response_message: None,
            bytes_sent: 0,
            bytes_received: 0,
            response_body: None,
            assertions: vec![],
        };
        if !crate::post_processor::run_pre(&self.pre_processors, context, &mut result).await {
            super::variables::clear(&self.config, &mut context.variables);
            result.elapsed = started.elapsed();
            return Ok(result);
        }
        if !crate::timer::run(&self.config.timers, context, &mut result).await {
            super::variables::clear(&self.config, &mut context.variables);
            return Ok(result);
        }
        let started = Instant::now();
        result.started_at = SystemTime::now();
        let expand = || -> Result<(String, String, Vec<Parameter>), Failure> {
            let expand = |text: &str| {
                template::expand(text, &context.variables).map_err(|e| Failure::config(e.message))
            };
            let label = expand(&self.config.label)?;
            let sql = expand(&self.config.sql)?;
            if sql.trim().is_empty() {
                return Err(Failure::config("SQL must not be empty"));
            }
            let parameters = self
                .config
                .parameters
                .iter()
                .map(|p| {
                    template::expand_json(p, &context.variables)
                        .map(Parameter::new)
                        .map_err(|e| Failure::config(e.message))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok((label, sql, parameters))
        };
        // A query can use its previous result as input before replacing it.
        let expanded = expand();
        super::variables::clear(&self.config, &mut context.variables);
        let output = match expanded {
            Err(error) => Err(error),
            Ok((label, sql, parameters)) => {
                result.label = label;
                result.bytes_sent =
                    sql.len() as u64 + parameters.iter().map(|p| p.len() as u64).sum::<u64>();
                tokio::time::timeout(
                    self.config.query_timeout,
                    self.execute(&parameters, &sql, &context.variables),
                )
                .await
                .unwrap_or_else(|_| Err(Failure("TIMEOUT".into(), "JDBC sample timed out".into())))
            }
        };
        let mut payload = None;
        match output {
            Ok(value) => {
                context.variables.extend(value.variables);
                result.bytes_received = value.body.len() as u64;
                result.success = true;
                result.response_code = Some("00000".into());
                result.response_message = Some("OK".into());
                payload = Some(value.body);
            }
            Err(Failure(code, message)) => {
                result.response_code = Some(code);
                result.response_message = Some(message);
            }
        }
        result.elapsed = started.elapsed();
        crate::post_processor::run(
            &self.post_processors,
            context,
            &mut result,
            payload.as_deref(),
            Some("complete JDBC result is unavailable"),
        )
        .await;
        let diagnostics = evaluate_assertions(
            &self.assertions,
            &AssertionInput {
                sample: &result,
                response_body: payload.as_deref(),
                unavailable_reason: payload
                    .is_none()
                    .then_some("complete JDBC result is unavailable"),
            },
        );
        for assertion in diagnostics {
            result.add_assertion(assertion);
        }
        if self.config.save_response_body {
            result.response_body = payload;
        }
        Ok(result)
    }
}

// A cancelled/timed-out/failed operation must never put an in-flight transaction
// back in the pool. Remove it, request server-side cancellation, then close it.
struct Lease {
    client: Option<Client>,
    tls: MakeTlsConnector,
    clean: bool,
}
impl Drop for Lease {
    fn drop(&mut self) {
        if !self.clean
            && let Some(client) = self.client.take()
        {
            let cancel = client.cancel_token();
            let client = Client::take(client);
            let tls = self.tls.clone();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    let _ = tokio::time::timeout(Duration::from_secs(1), cancel.cancel_query(tls))
                        .await;
                    drop(client);
                });
            } else {
                drop(client);
            }
        }
    }
}
