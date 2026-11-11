//! Synchronous JavaScript samples executed in resource-limited child processes.
use rmeter_core::{
    context::UserContext,
    sampler::{SampleError, SampleResult, Sampler},
};
use std::{
    collections::HashMap,
    fmt,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

mod engine;
mod process;
mod resource;

/// Set a trusted absolute path to the separately deployed JavaScript worker.
/// Call at host startup, before compiling plans. Repeating the same path is safe.
pub fn set_worker_executable(
    path: impl AsRef<std::path::Path>,
) -> Result<(), JavaScriptConfigError> {
    process::set_worker_executable(path.as_ref()).map_err(|e| JavaScriptConfigError(e.to_string()))
}

const MAX_SOURCE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct JavaScriptConfig {
    pub name: String,
    pub script: String,
    pub timeout: Duration,
    /// Total UTF-8 bytes in variable names and values, including initial variables.
    pub max_variable_bytes: usize,
    #[serde(default)]
    pub timers: Vec<crate::TimerConfig>,
    #[serde(default)]
    pub pre_processors: Vec<JavaScriptConfig>,
    #[serde(default)]
    pub post_processors: Vec<JavaScriptConfig>,
}

impl JavaScriptConfig {
    pub fn new(name: impl Into<String>, script: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            script: script.into(),
            timeout: Duration::from_millis(100),
            max_variable_bytes: 1024 * 1024,
            timers: Vec::new(),
            pre_processors: Vec::new(),
            post_processors: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub struct JavaScriptConfigError(pub String);
impl fmt::Display for JavaScriptConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for JavaScriptConfigError {}

#[derive(Debug, Clone)]
pub struct JavaScriptSampler {
    config: Arc<JavaScriptConfig>,
    pre_processors: Vec<JavaScriptSampler>,
    post_processors: Vec<JavaScriptSampler>,
}

impl JavaScriptSampler {
    pub fn new(config: JavaScriptConfig) -> Result<Self, JavaScriptConfigError> {
        let invalid = |message: &str| {
            JavaScriptConfigError(format!("JavaScript {:?}: {message}", config.name))
        };
        if config.name.trim().is_empty() {
            return Err(invalid("name must not be empty"));
        }
        if config.script.len() > MAX_SOURCE_BYTES {
            return Err(invalid("script exceeds 1 MiB"));
        }
        if config.timeout.is_zero() || config.timeout > Duration::from_secs(60) {
            return Err(invalid("timeout must be between 1 ns and 60 s"));
        }
        if config.max_variable_bytes == 0 {
            return Err(invalid("max_variable_bytes must be positive"));
        }
        crate::timer::validate(&config.timers).map_err(JavaScriptConfigError)?;
        let pre_processors = crate::post_processor::compile(&config.pre_processors)?;
        let post_processors = crate::post_processor::compile(&config.post_processors)?;
        let config = Arc::new(config);
        process::invoke(
            Request {
                config: config.clone(),
                variables: HashMap::new(),
                metadata: serde_json::json!({}),
                previous: None,
                check_only: true,
                extractor: None,
            },
            &AtomicBool::new(false),
        )
        .and_then(Response::into_variables)
        .map_err(|(_, message)| {
            JavaScriptConfigError(format!("JavaScript {:?}: {message}", config.name))
        })?;
        Ok(Self {
            config,
            pre_processors,
            post_processors,
        })
    }

    pub fn config(&self) -> &JavaScriptConfig {
        &self.config
    }

    /// Execute without producing a sample. Each invocation commits variables only
    /// on success; the caller owns the parent sample and its error policy.
    pub(crate) async fn run(
        &self,
        user: &mut UserContext,
        previous: Option<Arc<serde_json::Value>>,
        sampler_name: &str,
    ) -> Result<(), (&'static str, String)> {
        let config = self.config.clone();
        let variables = user.variables.clone();
        let metadata = serde_json::json!({
            "groupName": user.group_name, "userIndex": user.user_index,
            "iteration": user.iteration, "lastSampleOk": user.last_sample_ok,
            "samplerName": sampler_name,
        });
        let cancel = CancelOnDrop(Arc::new(AtomicBool::new(false)));
        let signal = cancel.0.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            process::invoke(
                Request {
                    config,
                    variables,
                    metadata,
                    previous,
                    check_only: false,
                    extractor: None,
                },
                &signal,
            )
        })
        .await
        .map_err(|e| ("Internal", e.to_string()))?;
        let response = outcome?;
        user.script_logs.extend(response.logs.clone());
        user.variables = response.into_variables()?;
        Ok(())
    }
}

// Dropping a sample signals the parent supervisor to kill and reap its child.
struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

impl Sampler for JavaScriptSampler {
    async fn sample(&self, user: &mut UserContext) -> Result<SampleResult, SampleError> {
        let started_at = SystemTime::now();
        let started = Instant::now();
        let mut pre_result = SampleResult {
            label: self.config.name.clone(),
            started_at,
            elapsed: Duration::ZERO,
            success: false,
            response_code: None,
            response_message: None,
            bytes_sent: 0,
            bytes_received: 0,
            response_body: None,
            assertions: Vec::new(),
        };
        if !crate::post_processor::run_pre(&self.pre_processors, user, &mut pre_result).await {
            pre_result.elapsed = started.elapsed();
            return Ok(pre_result);
        }
        if !crate::timer::run(&self.config.timers, user, &mut pre_result).await {
            return Ok(pre_result);
        }
        let started_at = SystemTime::now();
        let started = Instant::now();
        let (success, code, message) = match self.run(user, None, &self.config.name).await {
            Ok(()) => (true, "OK", None),
            Err((code, message)) => (false, code, Some(message)),
        };
        let mut result = SampleResult {
            label: self.config.name.clone(),
            started_at,
            elapsed: started.elapsed(),
            success,
            response_code: Some(code.into()),
            response_message: message,
            bytes_sent: 0,
            bytes_received: 0,
            response_body: None,
            assertions: Vec::new(),
        };
        crate::post_processor::run(
            &self.post_processors,
            user,
            &mut result,
            None,
            Some("JavaScript sampler has no response body"),
        )
        .await;
        Ok(result)
    }
}

type ExecutionResult = Result<HashMap<String, String>, (&'static str, String)>;

#[derive(serde::Serialize, serde::Deserialize)]
struct Request {
    config: Arc<JavaScriptConfig>,
    variables: HashMap<String, String>,
    metadata: serde_json::Value,
    previous: Option<Arc<serde_json::Value>>,
    check_only: bool,
    extractor: Option<crate::extractor::ExtractorConfig>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Response {
    variables: Option<HashMap<String, String>>,
    error: Option<(String, String)>,
    logs: rmeter_core::context::ScriptLogs,
}
impl Response {
    fn into_variables(self) -> ExecutionResult {
        if let Some((code, message)) = self.error {
            let code = match code.as_str() {
                "JavaScriptError" => "JavaScriptError",
                "Timeout" => "Timeout",
                "ExtractorError" => "ExtractorError",
                _ => "Internal",
            };
            return Err((code, message));
        }
        self.variables
            .ok_or_else(|| ("Internal", "worker response missing variables".into()))
    }
}

/// Entry point for the separately deployed rmeter-js-worker executable.
#[doc(hidden)]
pub fn worker_main() -> std::process::ExitCode {
    use std::io::{Read, Write};
    #[cfg(windows)]
    // SAFETY: sets process-local error reporting flags in the dedicated worker,
    // preventing a parser crash from opening a Windows error dialog.
    unsafe {
        use windows_sys::Win32::System::Diagnostics::Debug::{
            SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetErrorMode,
        };
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX);
    }
    let run = || -> Result<(), Box<dyn std::error::Error>> {
        let mut stdout = std::io::stdout().lock();
        stdout.write_all(process::READY)?;
        stdout.flush()?;
        // The parent attaches OS resource limits before it writes any input.
        let mut bytes = Vec::new();
        std::io::stdin()
            .take(process::MAX_IPC_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > process::MAX_IPC_BYTES {
            return Err("worker request too large".into());
        }
        let request: Request = serde_json::from_slice(&bytes)?;
        let mut logs = Default::default();
        let result = if let Some(config) = &request.extractor {
            (if request.check_only {
                crate::extractor::validate(config).map(|_| HashMap::new())
            } else {
                let body = request
                    .previous
                    .as_ref()
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                crate::extractor::execute(config, body, request.variables)
            })
            .map_err(|e| ("ExtractorError", e.chars().take(4096).collect()))
        } else {
            engine::execute(request, &mut logs)
        };
        let response = match result {
            Ok(variables) => Response {
                variables: Some(variables),
                error: None,
                logs,
            },
            Err((code, message)) => Response {
                variables: None,
                error: Some((code.into(), message)),
                logs,
            },
        };
        let bytes = serde_json::to_vec(&response)?;
        if bytes.len() > process::MAX_IPC_BYTES {
            return Err("worker response too large".into());
        }
        stdout.write_all(&bytes)?;
        stdout.flush()?;
        Ok(())
    };
    if run().is_ok() {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::from(2)
    }
}

pub(crate) fn check_extractor(
    extractor: crate::extractor::ExtractorConfig,
) -> Result<(), (&'static str, String)> {
    process::invoke(
        Request {
            config: Arc::new(crate::extractor::worker_config(&extractor)),
            variables: HashMap::new(),
            metadata: serde_json::json!({}),
            previous: None,
            check_only: true,
            extractor: Some(extractor),
        },
        &AtomicBool::new(false),
    )
    .and_then(Response::into_variables)
    .map(|_| ())
}
pub(crate) async fn run_extractor(
    extractor: Arc<crate::extractor::ExtractorConfig>,
    user: &mut UserContext,
    body: &str,
) -> Result<(), (&'static str, String)> {
    let request = Request {
        config: Arc::new(crate::extractor::worker_config(&extractor)),
        variables: user.variables.clone(),
        metadata: serde_json::json!({}),
        previous: Some(Arc::new(serde_json::Value::String(body.into()))),
        check_only: false,
        extractor: Some((*extractor).clone()),
    };
    let cancel = CancelOnDrop(Arc::new(AtomicBool::new(false)));
    let signal = cancel.0.clone();
    let variables = tokio::task::spawn_blocking(move || {
        process::invoke(request, &signal).and_then(Response::into_variables)
    })
    .await
    .map_err(|e| ("Internal", e.to_string()))??;
    let count = variables
        .get(&format!("{}_matchNr", extractor.settings.variable_name))
        .cloned()
        .unwrap_or_default();
    user.variables = variables;
    user.script_logs.push(rmeter_core::context::ScriptLog {
        level: "info".into(),
        source: extractor.settings.name.clone(),
        message: format!(
            "extracted {count} match(es) into {:?}",
            extractor.settings.variable_name
        ),
    });
    Ok(())
}
