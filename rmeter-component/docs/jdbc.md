# PostgreSQL sampling

The JDBC-style names follow the component model; the implementation uses native
Rust PostgreSQL drivers and does not load a JVM or Java JDBC JAR. Only PostgreSQL
is supported. Start with [`JdbcConnectionConfig`], [`JdbcConnection`] and
[`JdbcSamplerConfig`], or register connection configurations with
[`crate::ComponentCompiler`] for mixed plans.

## Connections and scopes

[`JdbcConnectionConfig::new`] defaults to at most 10 pooled connections, a
10-second connection timeout and a 10-second acquisition timeout. Pools initialize
lazily on first sampling. A sampler references `variable_name`, which identifies
a pool rather than a string in the user's variable map. Connection configuration
nodes do not produce sample events.

Global pools can be shared across thread groups. Local configurations apply to
siblings and descendants regardless of declaration position; child scopes can
shadow a pool name. Duplicate names in the same scope are rejected. URL and
credential templates expand on first use; a pool's connection identity cannot
change after initialization. Use separate names for distinct identities.

URLs accept `postgres://`, `postgresql://` and `jdbc:postgresql://`. Passwords can
come from `password` or the environment variable named by `password_env`, but not
both. Debug formatting redacts the URL and password. TLS is required by default,
including for `sslmode=prefer`; only explicit `sslmode=disable` permits plaintext.

## Parameterized query

```rust,no_run
use std::sync::Arc;
use rmeter_component::{JdbcConnection, JdbcConnectionConfig, JdbcSampler, JdbcSamplerConfig};
use rmeter_core::{context::UserContext, sampler::Sampler};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let pool = Arc::new(JdbcConnection::new(JdbcConnectionConfig::new(
    "app", "postgresql://localhost/app",
))?);
let mut config = JdbcSamplerConfig::new("Lookup", "app", "SELECT $1::text AS name");
config.parameters.push(serde_json::json!("Alice"));
let sampler = JdbcSampler::new(config, pool)?;
let mut user = UserContext::new("Users", 0);
let result = sampler.sample(&mut user).await
    .map_err(|error| format!("{error:?}"))?;
assert!(result.success, "{:?}", result.response_message);
# Ok(())
# }
```

Parameters bind to `$1`, `$2`, and so on independently of SQL text. Prefer bound
parameters for data values. The default query type is [`JdbcQueryType::Select`];
[`JdbcQueryType::Update`] reports affected rows. Defaults are a 30-second query
timeout, 1,000 rows and 8 MiB of result data, with response saving enabled.

The result JSON contains `columns`, `rows`, `row_count` and `rows_affected`.
Rows contain values in column order. `result_variable`, `variable_names` and
`row_count_variable` can export query results to user variables. Postprocessors
see the complete bounded JSON through `prev.body`, independently of report saving.
Preprocessors and timers run before SQL/parameter expansion; postprocessors run
before assertions. Failed assertions cannot turn a failed query into success,
and rolling back script variables cannot undo SQL that has already executed.

For translated component guides, see [`crate::translations`].
