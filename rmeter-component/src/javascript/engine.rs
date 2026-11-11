//! QuickJS parsing, values and callbacks stay inside the resource-limited worker.
use super::{ExecutionResult, Request};
use rquickjs::{
    CatchResultExt, Context, Ctx, Error, Exception, Function, IntoJs, Object, Runtime, Value,
    context::EvalOptions, qjs,
};
use std::{
    cell::RefCell,
    collections::HashMap,
    ffi::CString,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

const HEAP_BYTES: usize = 64 * 1024 * 1024;
const STACK_BYTES: usize = 256 * 1024;

struct Variables {
    values: HashMap<String, String>,
    bytes: usize,
    limit: usize,
    quota_exceeded: bool,
}

fn bind_variables<'js>(
    ctx: &Ctx<'js>,
    state: &Rc<RefCell<Variables>>,
) -> rquickjs::Result<Object<'js>> {
    let vars = Object::new(ctx.clone())?;
    let get = state.clone();
    vars.set(
        "get",
        Function::new(ctx.clone(), move |ctx: Ctx<'js>, key: String| {
            match get.borrow().values.get(&key) {
                Some(value) => value.into_js(&ctx),
                None => Ok(Value::new_null(ctx)),
            }
        })?,
    )?;
    let put = state.clone();
    let put = Function::new(
        ctx.clone(),
        move |ctx: Ctx<'js>, key: String, value: String| -> rquickjs::Result<()> {
            let mut state = put.borrow_mut();
            let old_bytes = state.values.get(&key).map_or(0, |v| key.len() + v.len());
            let bytes = state.bytes - old_bytes + key.len() + value.len();
            if bytes > state.limit {
                state.quota_exceeded = true;
                return Err(Exception::throw_range(&ctx, "max_variable_bytes exceeded"));
            }
            state.values.insert(key, value);
            state.bytes = bytes;
            Ok(())
        },
    )?;
    vars.set("put", put.clone())?;
    vars.set("set", put)?;
    let remove = state.clone();
    vars.set(
        "remove",
        Function::new(ctx.clone(), move |key: String| {
            let mut state = remove.borrow_mut();
            if let Some(value) = state.values.remove(&key) {
                state.bytes -= key.len() + value.len();
                true
            } else {
                false
            }
        })?,
    )?;
    let contains = state.clone();
    vars.set(
        "contains",
        Function::new(ctx.clone(), move |key: String| {
            contains.borrow().values.contains_key(&key)
        })?,
    )?;
    Ok(vars)
}

fn compile_only(ctx: &Ctx<'_>, source: &str) -> rquickjs::Result<()> {
    let len = source.len();
    let source = CString::new(source)?;
    // SAFETY: the context is active inside Context::with. Both strings are
    // NUL-terminated and live through JS_Eval; QuickJS owns the returned value
    // until it is freed below. COMPILE_ONLY never executes the script.
    unsafe {
        let raw = ctx.as_raw().as_ptr();
        let value = qjs::JS_Eval(
            raw,
            source.as_ptr(),
            len as _,
            c"script.js".as_ptr(),
            (qjs::JS_EVAL_TYPE_GLOBAL | qjs::JS_EVAL_FLAG_COMPILE_ONLY) as _,
        );
        let failed = qjs::JS_IsException(value);
        qjs::JS_FreeValue(raw, value);
        if failed {
            Err(Error::Exception)
        } else {
            Ok(())
        }
    }
}

fn script_error(error: impl std::fmt::Display) -> (&'static str, String) {
    (
        "JavaScriptError",
        error.to_string().chars().take(4096).collect(),
    )
}

fn internal(error: impl std::fmt::Display) -> (&'static str, String) {
    ("Internal", error.to_string())
}

pub(super) fn execute(
    request: Request,
    output_logs: &mut rmeter_core::context::ScriptLogs,
) -> ExecutionResult {
    let Request {
        config,
        variables,
        metadata,
        previous,
        check_only,
    } = request;
    let bytes = variables.iter().map(|(k, v)| k.len() + v.len()).sum();
    if bytes > config.max_variable_bytes {
        return Err((
            "JavaScriptError",
            "initial variables exceed max_variable_bytes".into(),
        ));
    }

    let runtime = Runtime::new().map_err(internal)?;
    runtime.set_memory_limit(HEAP_BYTES);
    runtime.set_max_stack_size(STACK_BYTES);
    let interrupted = Arc::new(AtomicBool::new(false));
    if !check_only {
        let deadline = Instant::now() + config.timeout;
        let interrupted = interrupted.clone();
        runtime.set_interrupt_handler(Some(Box::new(move || {
            let expired = Instant::now() >= deadline;
            if expired {
                interrupted.store(true, Ordering::Relaxed);
            }
            expired
        })));
    }
    // No module loader or host I/O is installed. The parent also enforces a hard
    // deadline and can terminate the worker if a native operation does not yield.
    let context = Context::full(&runtime).map_err(internal)?;
    if check_only {
        return context.with(|ctx| {
            compile_only(&ctx, &config.script)
                .catch(&ctx)
                .map_err(script_error)?;
            Ok(HashMap::new())
        });
    }

    let state = Rc::new(RefCell::new(Variables {
        values: variables,
        bytes,
        limit: config.max_variable_bytes,
        quota_exceeded: false,
    }));
    let logs = Rc::new(RefCell::new(rmeter_core::context::ScriptLogs::default()));
    let result = context.with(|ctx| {
        let setup = || -> rquickjs::Result<()> {
            let globals = ctx.globals();
            let freeze: Function = ctx.eval("Object.freeze")?;
            let vars = bind_variables(&ctx, &state)?;
            let iteration = metadata["iteration"].as_u64().unwrap_or(0).saturating_add(1);
            vars.set("getIteration", Function::new(ctx.clone(), move || iteration)?)?;
            let vars: Object = freeze.call((vars,))?;
            globals.prop("vars", vars)?;
            let info = ctx.json_parse(metadata.to_string())?;
            let decorate: Function = ctx.eval("(c) => { c.getThreadNum = () => c.userIndex; c.getVariables = () => vars; return Object.freeze(c); }")?;
            let info: Object = decorate.call((info,))?;
            globals.prop("ctx", info)?;
            globals.prop("Label", config.name.clone())?;
            let sampler: Object = ctx.eval("Object.freeze({name:ctx.samplerName, getName:()=>ctx.samplerName})")?;
            globals.prop("sampler", sampler)?;
            if let Some(previous) = previous {
                let snapshot = ctx.json_parse(previous.to_string())?;
                let decorate: Function = ctx.eval("(p) => { p.getResponseDataAsString=()=>p.body; p.getResponseCode=()=>p.responseCode; p.getResponseMessage=()=>p.responseMessage; p.getSampleLabel=()=>p.label; p.getTime=()=>p.elapsedMs; p.isSuccessful=()=>p.success; return Object.freeze(p); }")?;
                let snapshot: Object = decorate.call((snapshot,))?;
                globals.prop("prev", snapshot)?;
            }
            let sink = logs.clone();
            let source = config.name.clone();
            let write = Function::new(ctx.clone(), move |level: String, message: String| {
                sink.borrow_mut().push(rmeter_core::context::ScriptLog {
                    level, source: source.clone(), message,
                });
            })?;
            let make_log: Function = ctx.eval(r#"(write) => {
                const log = {};
                for (const level of ['trace','debug','info','warn','error']) {
                    log[level] = (message, ...args) => {
                        let i = 0;
                        const text = String(message).replace(/\{\}/g, () => i < args.length ? String(args[i++]) : '{}');
                        write(level, text + (i < args.length ? ' ' + args.slice(i).map(String).join(' ') : ''));
                    };
                }
                log.isDebugEnabled = () => true;
                log.isInfoEnabled = () => true;
                return Object.freeze(log);
            }"#)?;
            let log: Object = make_log.call((write,))?;
            globals.prop("log", log)?;
            Ok(())
        };
        setup().catch(&ctx).map_err(internal)?;

        let mut options = EvalOptions::default();
        options.strict = false;
        options.filename = Some("script.js".into());
        ctx.eval_with_options::<(), _>(config.script.as_bytes(), options)
            .catch(&ctx)
            .map_err(script_error)
    });
    *output_logs = std::mem::take(&mut *logs.borrow_mut());
    if interrupted.load(Ordering::Relaxed) {
        return Err(("Timeout", "JavaScript deadline exceeded".into()));
    }
    result?;
    let mut state = state.borrow_mut();
    if state.quota_exceeded {
        return Err(("JavaScriptError", "max_variable_bytes exceeded".into()));
    }
    Ok(std::mem::take(&mut state.values))
}
