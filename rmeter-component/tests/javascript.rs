use rmeter_component::{JavaScriptConfig, JavaScriptSampler};
use rmeter_core::{context::UserContext, sampler::Sampler};
use std::{collections::HashMap, time::Duration};

fn config(script: &str) -> JavaScriptConfig {
    let mut config = JavaScriptConfig::new("JS", script);
    config.timeout = Duration::from_secs(5);
    config
}

#[tokio::test]
async fn es6_variables_are_isolated_persistent_and_never_interpolated() {
    let sampler = JavaScriptSampler::new(config(
        r#"
        const [n] = [Number(vars.get('count') || '0') + 1];
        const format = v => `${ctx.groupName}-${ctx.userIndex}-${ctx.iteration}-${v}`;
        vars.set('count', String(n));
        vars.set('__proto__', format(n));
        vars.set('unicode', '中文😀');
        if (vars.get('missing') !== undefined) throw new Error('missing');
        if (!vars.remove('remove') || vars.remove('missing')) throw new Error('remove');
    "#,
    ))
    .unwrap();
    let mut a = UserContext::new("g", 0);
    a.variables.insert("remove".into(), "old".into());
    assert!(sampler.sample(&mut a).await.unwrap().success);
    assert_eq!(a.variables["count"], "1");
    assert_eq!(a.variables["__proto__"], "g-0-0-1");
    assert_eq!(a.variables["unicode"], "中文😀");
    a.iteration = 1;
    a.variables.insert("remove".into(), "old".into());
    assert!(sampler.sample(&mut a).await.unwrap().success);
    assert_eq!(a.variables["count"], "2");
    let mut b = UserContext::new("g", 1);
    b.variables.insert("remove".into(), "old".into());
    assert!(sampler.sample(&mut b).await.unwrap().success);
    assert_eq!(b.variables["count"], "1");
}

#[tokio::test]
async fn exceptions_and_invalid_types_rollback_all_changes() {
    for script in [
        "vars.set('x', 'changed'); vars.remove('old'); throw new Error('broken');",
        "vars.set('x', 'changed'); vars.set('bad', 123);",
        "vars.set('x', 'changed'); vars.get();",
        "vars.set('x', 'changed'); vars.set('bad', '\\ud800');",
    ] {
        let mut user = UserContext::new("g", 0);
        user.variables = HashMap::from([("old".into(), "value".into())]);
        let before = user.variables.clone();
        let result = JavaScriptSampler::new(config(script))
            .unwrap()
            .sample(&mut user)
            .await
            .unwrap();
        assert!(!result.success, "{script}");
        assert_eq!(result.label, "JS");
        assert_eq!(result.response_code.as_deref(), Some("JavaScriptError"));
        assert_eq!(user.variables, before);
    }
}

#[tokio::test]
async fn loop_recursion_stack_and_variable_limits_fail_without_committing() {
    for (script, limit) in [
        ("vars.set('x','new'); while (true) {}", 0),
        ("vars.set('x','new'); function f(){f()} f();", 1),
        ("vars.set('x','new'); function f(){f()} f();", 2),
        (
            "vars.set('x','new'); vars.set('large', 'x'.repeat(100));",
            3,
        ),
        (
            "vars.set('x','new'); try { vars.set('large', 'x'.repeat(100)); } catch(e) {}",
            3,
        ),
        // Runtime limit exceptions must not be swallowed by JavaScript catch.
        (
            "try { while(true) {} } catch(e) { vars.set('caught','yes'); }",
            0,
        ),
    ] {
        let mut config = config(script);
        match limit {
            0 => config.loop_iteration_limit = 20,
            1 => config.recursion_limit = 16,
            2 => config.stack_size_limit = 32,
            _ => config.max_variable_bytes = 32,
        }
        let mut user = UserContext::new("g", 0);
        let result = tokio::time::timeout(
            Duration::from_secs(10),
            JavaScriptSampler::new(config).unwrap().sample(&mut user),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!result.success, "{script}");
        assert!(user.variables.is_empty());
    }
}

#[tokio::test]
async fn timeout_stops_vm_and_leaves_async_executor_responsive() {
    let mut config = config("vars.set('x','new'); while(true) {}");
    config.timeout = Duration::from_millis(30);
    config.loop_iteration_limit = u64::MAX - 1;
    let sampler = JavaScriptSampler::new(config).unwrap();
    let mut user = UserContext::new("g", 0);
    let (sample, ()) = tokio::join!(sampler.sample(&mut user), async {
        tokio::time::sleep(Duration::from_millis(1)).await;
    });
    let result = sample.unwrap();
    assert!(!result.success);
    assert_eq!(result.response_code.as_deref(), Some("Timeout"));
    assert!(user.variables.is_empty());
}

#[tokio::test]
async fn context_and_globals_are_fresh_and_metadata_is_readonly() {
    let sampler = JavaScriptSampler::new(config(
        r#"
        if (typeof previous !== 'undefined') throw new Error('leaked realm');
        var previous = true;
        ctx.iteration = 999;
        vars = {};
        vars.set = () => { throw new Error('bridge overwritten'); };
        vars.set('iteration', String(ctx.iteration));
        Promise.resolve().then(() => vars.set('async', 'unexpected'));
    "#,
    ))
    .unwrap();
    let mut user = UserContext::new("g", 0);
    for _ in 0..2 {
        assert!(sampler.sample(&mut user).await.unwrap().success);
        assert_eq!(user.variables["iteration"], "0");
        assert!(!user.variables.contains_key("async"));
    }
}

#[tokio::test]
async fn variable_quota_counts_utf8_and_allows_replacing_and_removing_values() {
    let mut cfg = config("vars.set('k', 'a'); vars.remove('k'); vars.set('字', '值');");
    cfg.max_variable_bytes = 6;
    let mut user = UserContext::new("g", 0);
    user.variables.insert("k".into(), "12345".into());
    assert!(
        JavaScriptSampler::new(cfg.clone())
            .unwrap()
            .sample(&mut user)
            .await
            .unwrap()
            .success
    );
    assert_eq!(user.variables["字"], "值");
    cfg.max_variable_bytes = 5;
    let before = user.variables.clone();
    let result = JavaScriptSampler::new(cfg)
        .unwrap()
        .sample(&mut user)
        .await
        .unwrap();
    assert!(!result.success);
    assert!(
        result
            .response_message
            .unwrap()
            .contains("initial variables")
    );
    assert_eq!(user.variables, before);
}

#[test]
fn syntax_and_invalid_limits_are_rejected_before_execution() {
    assert!(JavaScriptSampler::new(config("const = ;")).is_err());
    for limit in 0..6 {
        let mut config = config("");
        match limit {
            0 => config.timeout = Duration::ZERO,
            1 => config.loop_iteration_limit = u64::MAX,
            2 => config.recursion_limit = 0,
            3 => config.stack_size_limit = 0,
            4 => config.max_variable_bytes = 0,
            _ => config.timeout = Duration::from_secs(61),
        }
        assert!(JavaScriptSampler::new(config).is_err());
    }
}
