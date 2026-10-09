use std::{
    collections::HashMap,
    net::Ipv6Addr,
    time::{Duration, Instant, SystemTime},
};

use reqwest::{
    Client, Request, Url,
    header::{CONTENT_TYPE, HeaderMap, HeaderName, HeaderValue},
};
use rmeter_core::{
    assertion::AssertionInput,
    context::UserContext,
    sampler::{SampleError, SampleResult, Sampler},
};

use crate::assertion::{JsonAssertion, evaluate_json_assertions};

use super::{HttpBody, HttpConfigError, HttpRequestConfig, HttpRequestDefaults, Setting, template};

/// Immutable, shareable sampler. Cookies are disabled; no user session state is shared.
/// Byte counters measure the initial request body and the final response body,
/// excluding headers, TLS, and intermediate redirects. Elapsed time includes redirects
/// and consumption of the complete final body, even when saving is disabled/truncated.
#[derive(Debug, Clone)]
pub struct HttpSampler {
    config: HttpRequestConfig,
    client: Client,
    assertions: Vec<JsonAssertion>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ClientOptions {
    connect_timeout: Option<Duration>,
    follow_redirects: bool,
    max_redirects: usize,
    use_system_proxy: bool,
}

impl HttpSampler {
    pub fn new(config: HttpRequestConfig) -> Result<Self, HttpConfigError> {
        Self::with_defaults(config, &HttpRequestDefaults::default())
    }

    pub fn with_defaults(
        config: HttpRequestConfig,
        defaults: &HttpRequestDefaults,
    ) -> Result<Self, HttpConfigError> {
        Self::prepare(config, defaults, &mut HashMap::new())
    }

    pub fn config(&self) -> &HttpRequestConfig {
        &self.config
    }

    pub(super) fn prepare(
        mut config: HttpRequestConfig,
        defaults: &HttpRequestDefaults,
        clients: &mut HashMap<ClientOptions, Client>,
    ) -> Result<Self, HttpConfigError> {
        config.settings = defaults.merged_with(&config.settings).effective();
        let assertions = config
            .assertions
            .iter()
            .cloned()
            .map(JsonAssertion::new)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| HttpConfigError::new(error.to_string()))?;
        let settings = &config.settings;
        if config.url.is_none() && settings.host.is_none() {
            return Err(HttpConfigError::new(
                "HTTP host or absolute URL is required",
            ));
        }
        for duration in [settings.connect_timeout, settings.request_timeout] {
            if let Setting::Value(Some(duration)) = duration
                && Instant::now().checked_add(duration).is_none()
            {
                return Err(HttpConfigError::new("HTTP timeout exceeds the clock range"));
            }
        }
        if settings.follow_redirects == Some(true) && settings.max_redirects == Some(0) {
            return Err(HttpConfigError::new(
                "max_redirects must be positive when redirects are enabled",
            ));
        }

        let strings = config_strings(&config);
        for text in &strings {
            template::validate(text)?;
        }
        let has_variables = strings.iter().any(|text| text.contains("${"));
        let options = ClientOptions {
            connect_timeout: match settings.connect_timeout {
                Setting::Value(value) => value,
                Setting::Inherit => unreachable!("effective settings contain concrete values"),
            },
            follow_redirects: settings.follow_redirects.unwrap(),
            max_redirects: settings.max_redirects.unwrap(),
            use_system_proxy: settings.use_system_proxy.unwrap(),
        };
        let client = if let Some(client) = clients.get(&options) {
            client.clone()
        } else {
            let mut builder = Client::builder()
                // Retries would distort load and may duplicate non-idempotent requests.
                .retry(reqwest::retry::never())
                .redirect(if options.follow_redirects {
                    reqwest::redirect::Policy::limited(options.max_redirects)
                } else {
                    reqwest::redirect::Policy::none()
                });
            if let Some(timeout) = options.connect_timeout {
                builder = builder.connect_timeout(timeout);
            }
            if !options.use_system_proxy {
                builder = builder.no_proxy();
            }
            let client = builder
                .build()
                .map_err(|error| HttpConfigError::new(error.to_string()))?;
            clients.insert(options, client.clone());
            client
        };
        let sampler = Self {
            config,
            client,
            assertions,
        };
        // Validate static requests before running. Values depending on user variables
        // are validated on each sample after expansion instead.
        if !has_variables {
            sampler.build_request(&HashMap::new())?;
        }
        Ok(sampler)
    }

    fn build_request(
        &self,
        variables: &HashMap<String, String>,
    ) -> Result<Request, HttpConfigError> {
        let settings = &self.config.settings;
        let url = build_url(&self.config, variables)?;
        let mut builder = self.client.request(self.config.method.clone(), url);
        let mut headers = HeaderMap::new();
        if let Some(entries) = &settings.headers {
            for (name, value) in entries {
                let name = template::expand(name, variables)?;
                let value = template::expand(value, variables)?;
                let name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
                    HttpConfigError::new(format!("invalid header name: {error}"))
                })?;
                let value = HeaderValue::from_str(&value).map_err(|error| {
                    HttpConfigError::new(format!("invalid header value: {error}"))
                })?;
                headers.append(name, value);
            }
        }
        if let Some(query) = &settings.query {
            builder = builder.query(&expand_pairs(query, variables)?);
        }
        if let Setting::Value(Some(timeout)) = settings.request_timeout {
            builder = builder.timeout(timeout);
        }
        match &self.config.body {
            HttpBody::Empty => {}
            HttpBody::Bytes(bytes) => {
                builder = builder.body(bytes.clone());
            }
            HttpBody::Text(text) => {
                if !headers.contains_key(CONTENT_TYPE) {
                    headers.insert(
                        CONTENT_TYPE,
                        HeaderValue::from_static("text/plain; charset=utf-8"),
                    );
                }
                builder = builder.body(template::expand(text, variables)?);
            }
            HttpBody::Form(pairs) => {
                builder = builder.form(&expand_pairs(pairs, variables)?);
            }
            HttpBody::Json(value) => {
                builder = builder.json(&template::expand_json(value, variables)?);
            }
        }
        // Explicit headers override automatically generated content types.
        builder
            .headers(headers)
            .build()
            .map_err(|error| HttpConfigError::new(error.to_string()))
    }

    fn apply_assertions(
        &self,
        result: &mut SampleResult,
        body: Option<&[u8]>,
        reason: Option<&str>,
    ) {
        let diagnostics = evaluate_json_assertions(
            &self.assertions,
            &AssertionInput {
                sample: result,
                response_body: body,
                unavailable_reason: reason,
            },
        );
        for assertion in diagnostics {
            result.add_assertion(assertion);
        }
    }

    fn failed_before_response(&self, mut result: SampleResult, started: Instant) -> SampleResult {
        result.elapsed = started.elapsed();
        self.apply_assertions(
            &mut result,
            None,
            Some("request failed before a complete response was available"),
        );
        result
    }
}

impl Sampler for HttpSampler {
    /// Operational and runtime configuration errors are unsuccessful SampleResults
    /// so that request labels and partial response metrics survive engine reporting.
    async fn sample(&self, context: &mut UserContext) -> Result<SampleResult, SampleError> {
        let started_at = SystemTime::now();
        let started = Instant::now();
        let mut result = SampleResult {
            label: self.config.label.clone(),
            started_at,
            elapsed: Duration::ZERO,
            success: false,
            response_code: None,
            response_message: None,
            bytes_sent: 0,
            bytes_received: 0,
            response_body: self
                .config
                .settings
                .save_response_body
                .unwrap()
                .then(Vec::new),
            assertions: Vec::new(),
        };
        match template::expand(&self.config.label, &context.variables) {
            Ok(label) => result.label = label,
            Err(error) => {
                result.response_message = Some(format!("InvalidConfig: {error}"));
                return Ok(self.failed_before_response(result, started));
            }
        }
        let request = match self.build_request(&context.variables) {
            Ok(request) => request,
            Err(error) => {
                result.response_message = Some(format!("InvalidConfig: {error}"));
                return Ok(self.failed_before_response(result, started));
            }
        };
        result.bytes_sent = request
            .body()
            .and_then(|body| body.as_bytes())
            .map_or(0, |body| body.len() as u64);
        let mut response = match self.client.execute(request).await {
            Ok(response) => response,
            Err(error) => {
                result.response_message = Some(describe_error(error));
                return Ok(self.failed_before_response(result, started));
            }
        };
        let status = response.status();
        result.response_code = Some(status.as_u16().to_string());
        result.response_message = status.canonical_reason().map(str::to_owned);
        result.success = status.is_success() || status.is_redirection();
        let mut assertion_body = (!self.assertions.is_empty()).then(Vec::new);
        let mut assertion_body_overflow = false;
        let mut response_complete = true;
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    result.bytes_received =
                        result.bytes_received.saturating_add(chunk.len() as u64);
                    if let Some(body) = &mut result.response_body {
                        let available = self
                            .config
                            .settings
                            .max_saved_body_bytes
                            .unwrap()
                            .saturating_sub(body.len());
                        body.extend_from_slice(&chunk[..chunk.len().min(available)]);
                    }
                    if let Some(body) = &mut assertion_body
                        && !assertion_body_overflow
                    {
                        if chunk.len()
                            > self
                                .config
                                .max_assertion_body_bytes
                                .saturating_sub(body.len())
                        {
                            assertion_body_overflow = true;
                            body.clear();
                        } else {
                            body.extend_from_slice(&chunk);
                        }
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    result.success = false;
                    response_complete = false;
                    result.response_message = Some(describe_error(error));
                    break;
                }
            }
        }
        result.elapsed = started.elapsed();
        let reason = if !response_complete {
            Some("response body is incomplete".to_owned())
        } else if assertion_body_overflow {
            Some(format!(
                "JSON assertion input exceeds {} bytes",
                self.config.max_assertion_body_bytes
            ))
        } else {
            None
        };
        self.apply_assertions(&mut result, assertion_body.as_deref(), reason.as_deref());
        Ok(result)
    }
}

fn describe_error(error: reqwest::Error) -> String {
    let kind = if error.is_timeout() {
        "Timeout"
    } else {
        "Transport"
    };
    format!("{kind}: {error}")
}

fn expand_pairs(
    entries: &[(String, String)],
    variables: &HashMap<String, String>,
) -> Result<Vec<(String, String)>, HttpConfigError> {
    entries
        .iter()
        .map(|(name, value)| {
            Ok((
                template::expand(name, variables)?,
                template::expand(value, variables)?,
            ))
        })
        .collect()
}

fn build_url(
    config: &HttpRequestConfig,
    variables: &HashMap<String, String>,
) -> Result<Url, HttpConfigError> {
    let mut url = if let Some(url) = &config.url {
        Url::parse(&template::expand(url, variables)?)
            .map_err(|error| HttpConfigError::new(format!("invalid HTTP URL: {error}")))?
    } else {
        let settings = &config.settings;
        let protocol = template::expand(settings.protocol.as_deref().unwrap(), variables)?;
        let host = template::expand(settings.host.as_deref().unwrap(), variables)?;
        if host.is_empty() || host.contains(['/', '?', '#', '@', '\\']) {
            return Err(HttpConfigError::new(
                "host must contain only a hostname or IP address",
            ));
        }
        let host = if host.parse::<Ipv6Addr>().is_ok() {
            format!("[{host}]")
        } else {
            host
        };
        let mut url = Url::parse(&format!("{protocol}://{host}/"))
            .map_err(|error| HttpConfigError::new(format!("invalid HTTP endpoint: {error}")))?;
        // Explicit ports belong to the port field, even if they equal the default.
        if (!host.starts_with('[') && host.contains(':'))
            || (host.starts_with('[') && !host.ends_with(']'))
        {
            return Err(HttpConfigError::new(
                "configure the port separately from the host",
            ));
        }
        if let Some(port) = settings.port {
            url.set_port(Some(port))
                .map_err(|_| HttpConfigError::new("invalid HTTP port"))?;
        }
        let path = template::expand(settings.path.as_deref().unwrap(), variables)?;
        if !path.starts_with('/') || path.contains('#') || path.contains('\\') {
            return Err(HttpConfigError::new(
                "HTTP path must start with '/' and contain no fragment or backslash",
            ));
        }
        let (path, query) = path
            .split_once('?')
            .map_or((path.as_str(), None), |(path, query)| (path, Some(query)));
        url.set_path(path);
        url.set_query(query);
        url
    };
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(HttpConfigError::new(
            "URL must use http or https and have a host",
        ));
    }
    url.set_fragment(None);
    Ok(url)
}

fn config_strings(config: &HttpRequestConfig) -> Vec<&str> {
    let mut strings = vec![config.label.as_str()];
    if let Some(url) = &config.url {
        strings.push(url);
    } else {
        strings.extend(
            [
                config.settings.protocol.as_deref(),
                config.settings.host.as_deref(),
                config.settings.path.as_deref(),
            ]
            .into_iter()
            .flatten(),
        );
    }
    for entries in [&config.settings.headers, &config.settings.query]
        .into_iter()
        .flatten()
    {
        for (name, value) in entries {
            strings.extend([name.as_str(), value.as_str()]);
        }
    }
    match &config.body {
        HttpBody::Text(text) => strings.push(text),
        HttpBody::Form(entries) => {
            for (name, value) in entries {
                strings.extend([name.as_str(), value.as_str()]);
            }
        }
        HttpBody::Json(value) => template::json_strings(value, &mut strings),
        _ => {}
    }
    strings
}
