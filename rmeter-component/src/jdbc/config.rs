use crate::AssertionConfig;
use serde_json::Value;
use std::{fmt, time::Duration};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JdbcConfigError {
    pub message: String,
}
impl JdbcConfigError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}
impl fmt::Display for JdbcConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for JdbcConfigError {}

#[derive(Clone)]
pub struct JdbcConnectionConfig {
    pub name: String,
    /// Stable identifier referenced by samplers; not a user-context string value.
    pub variable_name: String,
    /// postgres://, postgresql:// or jdbc:postgresql://; templates expand on first use.
    pub url: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub password_env: Option<String>,
    pub max_connections: usize,
    pub connect_timeout: Duration,
    pub acquire_timeout: Duration,
}
impl JdbcConnectionConfig {
    pub fn new(variable_name: impl Into<String>, url: impl Into<String>) -> Self {
        let variable_name = variable_name.into();
        Self {
            name: variable_name.clone(),
            variable_name,
            url: url.into(),
            username: None,
            password: None,
            password_env: None,
            max_connections: 10,
            connect_timeout: Duration::from_secs(10),
            acquire_timeout: Duration::from_secs(10),
        }
    }
}
// Connection URLs can also embed credentials. Never include them in Debug output.
impl fmt::Debug for JdbcConnectionConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JdbcConnectionConfig")
            .field("name", &self.name)
            .field("variable_name", &self.variable_name)
            .field("url", &"[redacted]")
            .field("password", &"[redacted]")
            .field("max_connections", &self.max_connections)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum JdbcQueryType {
    #[default]
    Select,
    Update,
}

#[derive(Debug, Clone)]
pub struct JdbcSamplerConfig {
    pub label: String,
    pub connection: String,
    pub query_type: JdbcQueryType,
    pub sql: String,
    /// Parameters bind to PostgreSQL $1, $2, ... separately from SQL text.
    pub parameters: Vec<Value>,
    pub query_timeout: Duration,
    pub max_rows: usize,
    pub max_result_bytes: usize,
    pub save_response_body: bool,
    pub result_variable: Option<String>,
    pub row_count_variable: Option<String>,
    pub assertions: Vec<AssertionConfig>,
    /// Run after assertions, with the complete result JSON even if report saving is off.
    pub post_processors: Vec<crate::JavaScriptConfig>,
}
impl JdbcSamplerConfig {
    pub fn new(
        label: impl Into<String>,
        connection: impl Into<String>,
        sql: impl Into<String>,
    ) -> Self {
        Self {
            label: label.into(),
            connection: connection.into(),
            query_type: JdbcQueryType::Select,
            sql: sql.into(),
            parameters: Vec::new(),
            query_timeout: Duration::from_secs(30),
            max_rows: 1000,
            max_result_bytes: 8 * 1024 * 1024,
            save_response_body: true,
            result_variable: None,
            row_count_variable: None,
            assertions: Vec::new(),
            post_processors: Vec::new(),
        }
    }
    pub fn add_assertion(&mut self, assertion: impl Into<AssertionConfig>) {
        self.assertions.push(assertion.into());
    }

    pub fn add_post_processor(&mut self, processor: crate::JavaScriptConfig) {
        self.post_processors.push(processor);
    }
}

pub(crate) fn valid_duration(value: Duration, name: &str) -> Result<(), JdbcConfigError> {
    if value.is_zero() || std::time::Instant::now().checked_add(value).is_none() {
        return Err(JdbcConfigError::new(format!(
            "{name} must be positive and within the clock range"
        )));
    }
    Ok(())
}
