use std::{fmt, time::Duration};

use reqwest::Method;

use crate::assertion::AssertionConfig;

/// Distinguishes inheritance from explicit values, including disabling a timeout.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Setting<T> {
    #[default]
    Inherit,
    Value(T),
}

impl<T: Clone> Setting<T> {
    fn overlay(&self, inner: &Self) -> Self {
        match inner {
            Self::Inherit => self.clone(),
            Self::Value(_) => inner.clone(),
        }
    }
}

/// A configuration component: it never executes a request or emits a sample.
/// None means inherit. An explicit empty header/query list clears inherited entries.
/// Nonempty lists override matching keys and retain other inherited entries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HttpRequestDefaults {
    pub protocol: Option<String>,
    /// Hostname or IP address without scheme, port, path, or user information.
    pub host: Option<String>,
    pub port: Option<u16>,
    /// A complete default path, not a prefix for a request's explicit path.
    pub path: Option<String>,
    pub headers: Option<Vec<(String, String)>>,
    pub query: Option<Vec<(String, String)>>,
    pub connect_timeout: Setting<Option<Duration>>,
    pub request_timeout: Setting<Option<Duration>>,
    pub follow_redirects: Option<bool>,
    pub max_redirects: Option<usize>,
    pub save_response_body: Option<bool>,
    pub max_saved_body_bytes: Option<usize>,
    /// Direct connections by default; opt in to environment/system proxies.
    pub use_system_proxy: Option<bool>,
}

impl HttpRequestDefaults {
    /// Applies an inner scope's explicitly configured fields over this scope.
    pub fn merged_with(&self, inner: &Self) -> Self {
        Self {
            protocol: inner.protocol.clone().or_else(|| self.protocol.clone()),
            host: inner.host.clone().or_else(|| self.host.clone()),
            port: inner.port.or(self.port),
            path: inner.path.clone().or_else(|| self.path.clone()),
            headers: merge_entries(&self.headers, &inner.headers, true),
            query: merge_entries(&self.query, &inner.query, false),
            connect_timeout: self.connect_timeout.overlay(&inner.connect_timeout),
            request_timeout: self.request_timeout.overlay(&inner.request_timeout),
            follow_redirects: inner.follow_redirects.or(self.follow_redirects),
            max_redirects: inner.max_redirects.or(self.max_redirects),
            save_response_body: inner.save_response_body.or(self.save_response_body),
            max_saved_body_bytes: inner.max_saved_body_bytes.or(self.max_saved_body_bytes),
            use_system_proxy: inner.use_system_proxy.or(self.use_system_proxy),
        }
    }

    pub(super) fn effective(&self) -> Self {
        Self {
            protocol: Some("http".into()),
            path: Some("/".into()),
            connect_timeout: Setting::Value(Some(Duration::from_secs(10))),
            request_timeout: Setting::Value(Some(Duration::from_secs(30))),
            follow_redirects: Some(true),
            max_redirects: Some(10),
            save_response_body: Some(false),
            max_saved_body_bytes: Some(1024 * 1024),
            use_system_proxy: Some(false),
            ..Self::default()
        }
        .merged_with(self)
    }
}

fn merge_entries(
    outer: &Option<Vec<(String, String)>>,
    inner: &Option<Vec<(String, String)>>,
    case_insensitive: bool,
) -> Option<Vec<(String, String)>> {
    let Some(inner) = inner else {
        return outer.clone();
    };
    if inner.is_empty() {
        return Some(Vec::new());
    }
    let mut entries = outer.clone().unwrap_or_default();
    entries.retain(|(name, _)| {
        !inner.iter().any(|(other, _)| {
            if case_insensitive {
                name.eq_ignore_ascii_case(other)
            } else {
                name == other
            }
        })
    });
    entries.extend(inner.iter().cloned());
    Some(entries)
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum HttpBody {
    #[default]
    Empty,
    /// UTF-8 text with variable substitution; defaults to text/plain.
    Text(String),
    /// Exact bytes without substitution or an implicit content type.
    Bytes(Vec<u8>),
    /// Ordered pairs, preserving repeated form keys.
    Form(Vec<(String, String)>),
    /// Variables are expanded in string values, then JSON is serialized safely.
    Json(serde_json::Value),
}

#[derive(Debug, Clone)]
pub struct HttpRequestConfig {
    pub label: String,
    /// An absolute URL overrides the protocol/host/port/path fields.
    pub url: Option<String>,
    pub method: Method,
    /// Explicit request settings override the surrounding Defaults.
    pub settings: HttpRequestDefaults,
    pub body: HttpBody,
    pub assertions: Vec<AssertionConfig>,
    /// Independent from report storage. Oversized bodies fail all response assertions;
    /// the sampler still drains the whole response and reports its full byte count.
    pub max_assertion_body_bytes: usize,
}

impl HttpRequestConfig {
    /// Attach an independent JSON, response-body regex, or XML assertion.
    pub fn add_assertion(&mut self, assertion: impl Into<AssertionConfig>) {
        self.assertions.push(assertion.into());
    }

    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            url: None,
            method: Method::GET,
            settings: HttpRequestDefaults::default(),
            body: HttpBody::Empty,
            assertions: Vec::new(),
            max_assertion_body_bytes: 8 * 1024 * 1024,
        }
    }

    pub fn with_url(label: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            url: Some(url.into()),
            ..Self::new(label)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpConfigError {
    pub message: String,
}

impl HttpConfigError {
    pub(super) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for HttpConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for HttpConfigError {}
