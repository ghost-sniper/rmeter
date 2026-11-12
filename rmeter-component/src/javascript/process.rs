use super::{Request, Response, resource};
use std::{
    io::{self, Read, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

pub(super) const READY: &[u8] = b"RMETER_JS_WORKER_V5\n";
pub(super) const MAX_IPC_BYTES: usize = 64 * 1024 * 1024;
const MAX_WORKERS: usize = 4;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(5);
const CHECK_TIMEOUT: Duration = Duration::from_secs(2);
static WORKERS: Mutex<usize> = Mutex::new(0);
static WORKER_PATH: OnceLock<PathBuf> = OnceLock::new();

pub(super) fn set_worker_executable(path: &std::path::Path) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(io::Error::other("JavaScript worker path must be absolute"));
    }
    let path = path.canonicalize()?;
    if !path.is_file() {
        return Err(io::Error::other("JavaScript worker must be a file"));
    }
    #[cfg(windows)]
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
    {
        return Err(io::Error::other("JavaScript worker must be an .exe file"));
    }
    if let Err(path) = WORKER_PATH.set(path)
        && WORKER_PATH.get() != Some(&path)
    {
        return Err(io::Error::other(
            "JavaScript worker path is already configured",
        ));
    }
    Ok(())
}

struct Slot;
impl Slot {
    fn acquire(cancel: &AtomicBool) -> Result<Self, (&'static str, String)> {
        let started = Instant::now();
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(("Cancelled", "JavaScript cancelled".into()));
            }
            let mut count = WORKERS
                .lock()
                .map_err(|_| ("Internal", "JavaScript worker limit unavailable".into()))?;
            if *count < MAX_WORKERS {
                *count += 1;
                return Ok(Self);
            }
            drop(count);
            if started.elapsed() > STARTUP_TIMEOUT {
                return Err(("Internal", "JavaScript worker queue is full".into()));
            }
            thread::sleep(Duration::from_millis(2));
        }
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        if let Ok(mut count) = WORKERS.lock() {
            *count -= 1;
        }
    }
}

struct Supervisor {
    child: Child,
    _limits: resource::Limits,
    io: Option<thread::JoinHandle<()>>,
}
impl Drop for Supervisor {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(io) = self.io.take() {
            let _ = io.join();
        }
    }
}

fn worker_path() -> io::Result<PathBuf> {
    if let Some(path) = WORKER_PATH.get() {
        return Ok(path.clone());
    }
    let name = format!("rmeter-js-worker{}", std::env::consts::EXE_SUFFIX);
    let path = if let Some(path) = std::env::var_os("RMETER_JS_WORKER") {
        let path = PathBuf::from(path);
        if !path.is_absolute() {
            return Err(io::Error::other(
                "RMETER_JS_WORKER must be an absolute executable path",
            ));
        }
        path
    } else {
        let executable = std::env::current_exe()?;
        let sibling = executable.with_file_name(&name);
        if sibling.is_file() {
            sibling
        } else {
            // Support Cargo integration-test executables in target/{profile}/deps
            // and test-runner wrappers; never search PATH or the working directory.
            executable.ancestors().find(|p| {
                matches!(p.file_name().and_then(|s| s.to_str()), Some("debug" | "release"))
                    && p.parent().and_then(|p| p.file_name()).is_some_and(|s| s == "target")
            }).map(|p| p.join(&name)).filter(|p| p.is_file())
                .ok_or_else(|| io::Error::other("rmeter-js-worker is missing; deploy it beside the host or set RMETER_JS_WORKER"))?
        }
    };
    #[cfg(windows)]
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("exe"))
    {
        return Err(io::Error::other("JavaScript worker must be an .exe file"));
    }
    path.canonicalize()
}

pub(super) fn invoke(
    request: Request,
    cancel: &AtomicBool,
) -> Result<Response, (&'static str, String)> {
    let _slot = Slot::acquire(cancel)?;
    let timeout = if request.check_only {
        CHECK_TIMEOUT
    } else {
        request.config.timeout
    };
    let variable_limit = request.config.max_variable_bytes;
    let variables_size: usize = request
        .variables
        .iter()
        .map(|(k, v)| k.len() + v.len())
        .sum();
    if variables_size > variable_limit {
        return Err((
            "JavaScriptError",
            "initial variables exceed max_variable_bytes".into(),
        ));
    }
    let bytes = encode_request(&request).map_err(internal)?;
    let mut command = Command::new(worker_path().map_err(internal)?);
    command
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    resource::prepare(&mut command).map_err(internal)?;
    let mut child = command.spawn().map_err(internal)?;
    let limits = match resource::attach(&child) {
        Ok(limits) => limits,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(internal(error));
        }
    };
    let mut stdin = child.stdin.take().expect("piped stdin");
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut supervisor = Supervisor {
        child,
        _limits: limits,
        io: None,
    };
    let (ready_tx, ready_rx) = mpsc::channel();
    let (output_tx, output_rx) = mpsc::channel();
    supervisor.io = Some(
        thread::Builder::new()
            .name("rmeter-js-ipc".into())
            .spawn(move || {
                let result = (|| -> io::Result<Vec<u8>> {
                    let mut magic = [0; READY.len()];
                    stdout.read_exact(&mut magic)?;
                    if magic != READY {
                        return Err(io::Error::other("JavaScript worker protocol mismatch"));
                    }
                    let _ = ready_tx.send(());
                    stdin.write_all(&bytes)?;
                    drop(stdin);
                    let mut response = Vec::new();
                    stdout
                        .take(MAX_IPC_BYTES as u64 + 1)
                        .read_to_end(&mut response)?;
                    if response.len() > MAX_IPC_BYTES {
                        return Err(io::Error::other("JavaScript worker response too large"));
                    }
                    Ok(response)
                })();
                let _ = output_tx.send(result);
            })
            .map_err(internal)?,
    );
    let mut deadline = Instant::now() + STARTUP_TIMEOUT;
    let mut output = None;
    loop {
        if ready_rx.try_recv().is_ok() {
            deadline = Instant::now() + timeout;
        }
        if cancel.load(Ordering::Relaxed) {
            return Err((
                "Cancelled",
                "JavaScript cancelled; worker terminated".into(),
            ));
        }
        if Instant::now() >= deadline {
            return Err((
                "Timeout",
                "JavaScript deadline exceeded; worker terminated".into(),
            ));
        }
        if let Some(status) = supervisor.child.try_wait().map_err(internal)? {
            if !status.success() {
                return Err((
                    "JavaScriptError",
                    "isolated JavaScript worker crashed or exceeded its resource limit".into(),
                ));
            }
            if let Some(io) = supervisor.io.take() {
                io.join()
                    .map_err(|_| ("Internal", "worker IPC thread failed".into()))?;
            }
            let bytes = match output.take() {
                Some(bytes) => bytes,
                None => output_rx.try_recv().map_err(internal)?.map_err(internal)?,
            };
            let response: Response = serde_json::from_slice(&bytes).map_err(internal)?;
            let size: usize = response
                .variables
                .as_ref()
                .map_or(0, |vars| vars.iter().map(|(k, v)| k.len() + v.len()).sum());
            if size > variable_limit {
                return Err((
                    "JavaScriptError",
                    "worker variables exceed max_variable_bytes".into(),
                ));
            }
            return Ok(response);
        }
        // A failed IPC operation must also terminate a still-running child.
        if output.is_none() {
            match output_rx.try_recv() {
                Ok(Err(error)) => return Err(internal(error)),
                Ok(Ok(bytes)) => output = Some(bytes),
                _ => {}
            }
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn internal(error: impl std::fmt::Display) -> (&'static str, String) {
    (
        "Internal",
        format!("JavaScript worker process isolation failed: {error}"),
    )
}

fn encode_request(request: &Request) -> io::Result<Vec<u8>> {
    struct LimitedBuffer(Vec<u8>);
    impl Write for LimitedBuffer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > MAX_IPC_BYTES.saturating_sub(self.0.len()) {
                return Err(io::Error::other(
                    "JavaScript request exceeds 64 MiB IPC limit",
                ));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut buffer = LimitedBuffer(Vec::new());
    serde_json::to_writer(&mut buffer, request)?;
    Ok(buffer.0)
}
