//! Boa values and parsing are confined to the worker process.
use super::ExecutionResult;
use boa_engine::{
    Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction, Script, Source, js_string,
    module::IdleModuleLoader,
    object::{IntegrityLevel, ObjectInitializer},
    property::Attribute,
};
use std::{cell::RefCell, collections::HashMap};

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

pub(super) fn execute(request: super::Request) -> ExecutionResult {
    let super::Request {
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
    if check_only {
        script
            .codeblock(&mut context)
            .map_err(|e| ("JavaScriptError", e.to_string()))?;
        return Ok(HashMap::new());
    }
    script
        .evaluate(&mut context)
        .map_err(|e| ("JavaScriptError", e.to_string()))?;
    let variables = context
        .remove_data::<RefCell<Variables>>()
        .expect("variable bridge")
        .into_inner();
    if variables.quota_exceeded {
        return Err(("JavaScriptError", "max_variable_bytes exceeded".into()));
    }
    Ok(variables.values)
}
