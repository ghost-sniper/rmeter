//! Synchronous JavaScript samples. Only `vars` survives successful executions;
//! each invocation gets a fresh Boa realm on a blocking worker.
use std::{
    cell::RefCell,
    collections::HashMap,
    fmt,
    future::Future,
    pin::pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context as TaskContext, Poll},
    time::{Duration, Instant, SystemTime},
};

use boa_engine::{
    Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction, Script, Source, js_string,
    module::IdleModuleLoader,
    object::{IntegrityLevel, ObjectInitializer},
    property::Attribute,
};
use rmeter_core::{
    context::UserContext,
    sampler::{SampleError, SampleResult, Sampler},
};

const MAX_SOURCE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone)]
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
        // Check syntax without executing user code, including in CLI --check mode.
        let mut context = Context::default();
        Script::parse(Source::from_bytes(&config.script), None, &mut context)
            .and_then(|script| script.codeblock(&mut context))
            .map_err(|e| invalid(&e.to_string()))?;
        Ok(Self {
            config: Arc::new(config),
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
            execute(&config, variables, metadata, previous, &signal)
        })
        .await
        .map_err(|e| ("Internal", e.to_string()))?;
        user.variables = outcome?;
        Ok(())
    }
}

// Dropping a cancelled sample tells its worker to discard the realm at the next
// VM budget boundary. No Boa objects cross a thread or an await boundary.
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

struct Variables {
    values: HashMap<String, String>,
    bytes: usize,
    limit: usize,
    quota_exceeded: bool,
}

fn string_arg(args: &[JsValue], index: usize) -> JsResult<String> {
    args.get(index)
        .and_then(JsValue::as_string)
        .ok_or_else(|| {
            JsNativeError::typ()
                .with_message("vars arguments must be strings")
                .into()
        })
        .and_then(|s| {
            s.to_std_string().map_err(|_| {
                JsNativeError::typ()
                    .with_message("vars arguments must contain valid Unicode")
                    .into()
            })
        })
}

fn get_variable(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let key = string_arg(args, 0)?;
    let state = context
        .get_data::<RefCell<Variables>>()
        .expect("variable bridge")
        .borrow();
    Ok(state
        .values
        .get(&key)
        .map_or_else(JsValue::undefined, |v| JsString::from(v.as_str()).into()))
}

fn set_variable(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let key = string_arg(args, 0)?;
    let value = string_arg(args, 1)?;
    let mut state = context
        .get_data::<RefCell<Variables>>()
        .expect("variable bridge")
        .borrow_mut();
    let old_bytes = state.values.get(&key).map_or(0, |v| key.len() + v.len());
    let bytes = state
        .bytes
        .saturating_sub(old_bytes)
        .saturating_add(key.len())
        .saturating_add(value.len());
    if bytes > state.limit {
        state.quota_exceeded = true;
        return Err(JsNativeError::range()
            .with_message("max_variable_bytes exceeded")
            .into());
    }
    state.values.insert(key, value);
    state.bytes = bytes;
    Ok(JsValue::undefined())
}

fn remove_variable(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let key = string_arg(args, 0)?;
    let mut state = context
        .get_data::<RefCell<Variables>>()
        .expect("variable bridge")
        .borrow_mut();
    let removed = state.values.remove(&key);
    if let Some(value) = &removed {
        state.bytes -= key.len() + value.len();
    }
    Ok(removed.is_some().into())
}

type ExecutionResult = Result<HashMap<String, String>, (&'static str, String)>;

fn execute(
    config: &JavaScriptConfig,
    variables: HashMap<String, String>,
    metadata: serde_json::Value,
    previous: Option<Arc<serde_json::Value>>,
    cancel: &AtomicBool,
) -> ExecutionResult {
    let started = Instant::now();
    if cancel.load(Ordering::Relaxed) {
        return Err(("Cancelled", "JavaScript cancelled".into()));
    }
    let bytes = variables.iter().map(|(k, v)| k.len() + v.len()).sum();
    if bytes > config.max_variable_bytes {
        return Err((
            "JavaScriptError",
            "initial variables exceed max_variable_bytes".into(),
        ));
    }
    // Do not let Boa's default file module loader access the working directory.
    let mut context = Context::builder()
        .module_loader(std::rc::Rc::new(IdleModuleLoader))
        .build()
        .map_err(|e| ("Internal", e.to_string()))?;
    let limits = context.runtime_limits_mut();
    limits.set_loop_iteration_limit(config.loop_iteration_limit);
    limits.set_recursion_limit(config.recursion_limit);
    limits.set_stack_size_limit(config.stack_size_limit);
    context.insert_data(RefCell::new(Variables {
        values: variables,
        bytes,
        limit: config.max_variable_bytes,
        quota_exceeded: false,
    }));
    let setup = |context: &mut Context| -> JsResult<()> {
        let vars = ObjectInitializer::new(context)
            .function(
                NativeFunction::from_fn_ptr(get_variable),
                js_string!("get"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(set_variable),
                js_string!("set"),
                2,
            )
            .function(
                NativeFunction::from_fn_ptr(remove_variable),
                js_string!("remove"),
                1,
            )
            .build();
        vars.set_integrity_level(IntegrityLevel::Frozen, context)?;
        context.register_global_property(js_string!("vars"), vars, Attribute::READONLY)?;
        let ctx = JsValue::from_json(&metadata, context)?;
        ctx.as_object()
            .expect("metadata object")
            .set_integrity_level(IntegrityLevel::Frozen, context)?;
        context.register_global_property(js_string!("ctx"), ctx, Attribute::READONLY)?;
        if let Some(previous) = &previous {
            let prev = JsValue::from_json(previous, context)?;
            prev.as_object()
                .expect("sample result object")
                .set_integrity_level(IntegrityLevel::Frozen, context)?;
            context.register_global_property(js_string!("prev"), prev, Attribute::READONLY)?;
        }
        Ok(())
    };
    setup(&mut context).map_err(|e| ("Internal", e.to_string()))?;
    let script = Script::parse(Source::from_bytes(&config.script), None, &mut context)
        .map_err(|e| ("JavaScriptError", e.to_string()))?;
    {
        // This is VM scheduling only; JavaScript still runs synchronously and
        // promise jobs are deliberately never drained.
        let mut evaluation = pin!(script.evaluate_async_with_budget(&mut context, 256));
        let mut task = TaskContext::from_waker(futures_util::task::noop_waker_ref());
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(("Cancelled", "JavaScript cancelled".into()));
            }
            if started.elapsed() >= config.timeout {
                return Err(("Timeout", "JavaScript timeout exceeded".into()));
            }
            if let Poll::Ready(result) = evaluation.as_mut().poll(&mut task) {
                result.map_err(|e| ("JavaScriptError", e.to_string()))?;
                break;
            }
        }
    }
    if started.elapsed() >= config.timeout {
        return Err(("Timeout", "JavaScript timeout exceeded".into()));
    }
    let variables = context
        .remove_data::<RefCell<Variables>>()
        .expect("variable bridge")
        .into_inner();
    if variables.quota_exceeded {
        return Err(("JavaScriptError", "max_variable_bytes exceeded".into()));
    }
    Ok(variables.values)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropping_sample_guard_cancels_worker_without_waiting_for_timeout() {
        let cancel = CancelOnDrop(Arc::new(AtomicBool::new(false)));
        let signal = cancel.0.clone();
        let (entered, ready) = std::sync::mpsc::channel();
        let (finished, done) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut config = JavaScriptConfig::new("cancel", "while (true) {}");
            config.timeout = Duration::from_secs(60);
            config.loop_iteration_limit = u64::MAX - 1;
            entered.send(()).unwrap();
            let result = execute(
                &config,
                HashMap::new(),
                serde_json::json!({}),
                None,
                &signal,
            );
            finished.send(result).unwrap();
        });
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        // Whether still parsing or already executing, cancellation must be seen
        // without waiting for the script's 60-second execution timeout.
        drop(cancel);
        let result = done.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(result.unwrap_err().0, "Cancelled");
        worker.join().unwrap();
    }
}
