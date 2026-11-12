use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod, Runtime};
use postgres_native_tls::MakeTlsConnector;
use std::{fmt, sync::Mutex};
use tokio_postgres::{Config, config::SslMode};

use super::{JdbcConfigError, JdbcConnectionConfig, config::valid_duration};
use crate::http::template;

#[derive(PartialEq, Eq)]
struct Credentials {
    url: String,
    username: Option<String>,
    password: Option<String>,
}
struct ResolvedPool {
    credentials: Credentials,
    pool: Pool,
    tls: MakeTlsConnector,
}

/// A lazily initialized named pool shared by all samplers referencing this component.
pub struct JdbcConnection {
    config: JdbcConnectionConfig,
    resolved: Mutex<Option<ResolvedPool>>,
}
impl fmt::Debug for JdbcConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JdbcConnection")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}
impl JdbcConnection {
    pub fn new(config: JdbcConnectionConfig) -> Result<Self, JdbcConfigError> {
        if config.name.trim().is_empty()
            || config.variable_name.trim().is_empty()
            || config.variable_name.contains("${")
        {
            return Err(JdbcConfigError::new(
                "JDBC connection name and static variable_name are required",
            ));
        }
        if config.max_connections == 0 || config.max_connections > 65535 {
            return Err(JdbcConfigError::new(
                "JDBC max_connections must be between 1 and 65535",
            ));
        }
        valid_duration(config.connect_timeout, "connect_timeout")?;
        valid_duration(config.acquire_timeout, "acquire_timeout")?;
        if config.password.is_some() && config.password_env.is_some() {
            return Err(JdbcConfigError::new(
                "use password or password_env, not both",
            ));
        }
        for value in [
            Some(config.url.as_str()),
            config.username.as_deref(),
            config.password.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            template::validate(value).map_err(|e| JdbcConfigError::new(e.message))?;
        }
        if config
            .password_env
            .as_ref()
            .is_some_and(|name| name.trim().is_empty())
        {
            return Err(JdbcConfigError::new("password_env must not be empty"));
        }
        if !config.url.contains("${") {
            parse_url(&config.url)?;
        }
        Ok(Self {
            config,
            resolved: Mutex::new(None),
        })
    }
    pub fn config(&self) -> &JdbcConnectionConfig {
        &self.config
    }
    pub fn shares_pool_with(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }

    pub(super) fn pool(
        &self,
        variables: &mut rmeter_core::context::UserContext,
    ) -> Result<(Pool, MakeTlsConnector), JdbcConfigError> {
        let mut expand =
            |s: &str| template::expand(s, variables).map_err(|e| JdbcConfigError::new(e.message));
        let password = if let Some(name) = &self.config.password_env {
            Some(std::env::var(name).map_err(|_| {
                JdbcConfigError::new(format!("missing password environment variable {name:?}"))
            })?)
        } else {
            self.config
                .password
                .as_deref()
                .map(&mut expand)
                .transpose()?
        };
        let credentials = Credentials {
            url: expand(&self.config.url)?,
            username: self
                .config
                .username
                .as_deref()
                .map(&mut expand)
                .transpose()?,
            password,
        };
        let mut state = self
            .resolved
            .lock()
            .map_err(|_| JdbcConfigError::new("JDBC pool state is unavailable"))?;
        if let Some(resolved) = state.as_ref() {
            if credentials != resolved.credentials {
                return Err(JdbcConfigError::new(
                    "JDBC connection settings changed after pool initialization; use a separate connection configuration",
                ));
            }
            return Ok((resolved.pool.clone(), resolved.tls.clone()));
        }
        let mut pg = parse_url(&credentials.url)?;
        if let Some(username) = &credentials.username {
            pg.user(username);
        }
        if let Some(password) = &credentials.password {
            pg.password(password);
        }
        pg.connect_timeout(self.config.connect_timeout);
        pg.application_name("rmeter");
        let tls = MakeTlsConnector::new(
            native_tls::TlsConnector::new()
                .map_err(|_| JdbcConfigError::new("cannot initialize JDBC TLS connector"))?,
        );
        let manager = Manager::from_config(
            pg,
            tls.clone(),
            ManagerConfig {
                recycling_method: RecyclingMethod::Clean,
            },
        );
        let pool = Pool::builder(manager)
            .runtime(Runtime::Tokio1)
            .max_size(self.config.max_connections)
            .wait_timeout(Some(self.config.acquire_timeout))
            .create_timeout(Some(self.config.connect_timeout))
            .recycle_timeout(Some(self.config.acquire_timeout))
            .build()
            .map_err(|_| JdbcConfigError::new("cannot create JDBC connection pool"))?;
        *state = Some(ResolvedPool {
            credentials,
            pool: pool.clone(),
            tls: tls.clone(),
        });
        Ok((pool, tls))
    }
}

fn parse_url(url: &str) -> Result<Config, JdbcConfigError> {
    let url = url.strip_prefix("jdbc:").unwrap_or(url);
    if !url.starts_with("postgresql://") && !url.starts_with("postgres://") {
        return Err(JdbcConfigError::new(
            "only PostgreSQL URLs are supported by the JDBC component",
        ));
    }
    let mut config: Config = url
        .parse()
        .map_err(|_| JdbcConfigError::new("invalid PostgreSQL connection URL"))?;
    // The driver's default Prefer permits an unauthenticated server to reject
    // TLS, then request a cleartext password. Only an explicit disable opts out.
    if config.get_ssl_mode() != SslMode::Disable {
        config.ssl_mode(SslMode::Require);
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_is_required_unless_explicitly_disabled() {
        for url in [
            "postgresql://user@localhost/db",
            "postgres://user@localhost/db?sslmode=prefer",
            "jdbc:postgresql://localhost/db?sslmode=require",
        ] {
            assert_eq!(parse_url(url).unwrap().get_ssl_mode(), SslMode::Require);
        }
        assert_eq!(
            parse_url("postgresql://localhost/db?sslmode=disable")
                .unwrap()
                .get_ssl_mode(),
            SslMode::Disable
        );
    }
}
