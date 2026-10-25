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
    pub loop_iteration_limit: u64,
    pub recursion_limit: usize,
    pub stack_size_limit: usize,
    /// Total UTF-8 bytes in variable names and values, including initial variables.
    pub max_variable_bytes: usize,
}

impl JavaScriptConfig {
    pub fn new(name: impl Into<String>, script: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            script: script.into(),
            timeout: Duration::from_millis(100),
            loop_iteration_limit: 100_000,
            recursion_limit: 128,
            stack_size_limit: 16_384,
            max_variable_bytes: 1024 * 1024,
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
        if config.loop_iteration_limit == 0
            || config.loop_iteration_limit == u64::MAX
            || config.recursion_limit == 0
            || config.stack_size_limit == 0
            || config.max_variable_bytes == 0
        {
            return Err(invalid("execution limits must be positive and finite"));
        }
        let config = Arc::new(config);
        process::invoke(
            Request {
                config: config.clone(),
                variables: HashMap::new(),
                metadata: serde_json::json!({}),
                previous: None,
                check_only: true,
            },
            &AtomicBool::new(false),
        )
        .map_err(|(_, message)| {
            JavaScriptConfigError(format!("JavaScript {:?}: {message}", config.name))
        })?;
        Ok(Self { config })
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
    ) -> Result<(), (&'static str, String)> {
        let config = self.config.clone();
        let variables = user.variables.clone();
        let metadata = serde_json::json!({
            "groupName": user.group_name, "userIndex": user.user_index,
            "iteration": user.iteration, "lastSampleOk": user.last_sample_ok,
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
                },
                &signal,
            )
        })
        .await
        .map_err(|e| ("Internal", e.to_string()))?;
        user.variables = outcome?;
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
        let (success, code, message) = match self.run(user, None).await {
            Ok(()) => (true, "OK", None),
            Err((code, message)) => (false, code, Some(message)),
        };
        Ok(SampleResult {
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
        })
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
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Response {
    variables: Option<HashMap<String, String>>,
    error: Option<(String, String)>,
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
        let result = engine::execute(request);
        let response = match result {
            Ok(variables) => Response {
                variables: Some(variables),
                error: None,
            },
            Err((code, message)) => Response {
                variables: None,
                error: Some((code.into(), message)),
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
