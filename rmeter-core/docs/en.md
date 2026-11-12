# rmeter-core

**English** · [简体中文](crate::translations::zh_cn) · [繁體中文](crate::translations::zh_tw) · [日本語](crate::translations::ja) · [Français](crate::translations::fr) · [Deutsch](crate::translations::de)

The crate home page and API reference default to English. These guides are also available in five other languages; Rust identifiers and API reference comments remain English.

## Overview

`rmeter-core` defines protocol-independent execution plans, virtual-user state, sampling and assertion traits, controllers, and a Tokio engine. Use this crate when implementing a sampler or constructing a typed plan; use `rmeter` to load JSON scripts and `rmeter-component` for built-in transports.

## Quick start

This self-contained example traverses a conditional plan without network access. `Node::Sample` can hold any value for traversal; executing a plan with `Engine` requires its sample type to implement `Sampler`.

```rust
use rmeter_core::{
    context::UserContext,
    controller::{CursorStep, ExecutionCursor, IfController},
    plan::Node,
};

let nodes = vec![Node::If(IfController::new(
    "Enabled", "${enabled}", vec![Node::Sample("health")],
))];
let mut user = UserContext::new("Users", 0);
user.variables.insert("enabled".into(), "true".into());
let mut cursor = ExecutionCursor::new(&nodes);
assert!(matches!(cursor.step(&mut user).unwrap(), CursorStep::Sample(&"health")));
assert!(matches!(cursor.step(&mut user).unwrap(), CursorStep::Done));
```

## Execution and errors

Create a [`TestPlan`](crate::plan::TestPlan) with [`ThreadGroup`](crate::plan::ThreadGroup) entries, then call [`Engine::run`](crate::engine::Engine::run) inside Tokio while draining its bounded event channel concurrently. Each user has its own variables and controller cursor. User and group-iteration indices start at zero; variables survive subsequent samples and iterations for that user.

[`ErrorPolicy`](crate::plan::ErrorPolicy) selects continue, next iteration, stop user or stop test after a failed sample. Infrastructure and condition-evaluation failures return [`EngineError`](crate::engine::EngineError). Passing assertions cannot repair a failed sample. Dropping the run future aborts user tasks.

IF expressions accept only case-insensitive `true`. String While conditions stop on case-insensitive `false`; blank and `LAST` have special last-sample semantics. When using [`ExecutionCursor`](crate::controller::ExecutionCursor) directly, yield to the async runtime on `CursorStep::Yield` so control-only infinite loops remain cancellable.

## API map

- [`plan`](crate::plan): immutable plan definitions and error policies.
- [`sampler`](crate::sampler) / [`assertion`](crate::assertion): extension traits and results.
- [`context`](crate::context): per-user variables and bounded script logs.
- [`controller`](crate::controller): sequence, IF, While and traversal.
- [`engine`](crate::engine) / [`event`](crate::event): concurrent execution and event delivery.

## Native functions

The five functions RandomString, changeCase, dateTimeConvert, Random and RandomDate are shared by Rust, JavaScript (`functions`) and `${__function(...)}` expressions. Date patterns follow JDK 21 ISO/SMART rules using bundled locale and timezone data; runtime Java is not needed. Defaults use the system timezone and locale captured at run start. Invalid calls fail explicitly. Optional variable names store results for the current user; failed expressions roll back their writes. JavaScript source is never interpolated. Controller evaluation now takes a mutable UserContext to support function writes.

## Per-iteration controller timeouts

Loop and While use a new budget on each iteration. JSON `timeout_ms` is a nonnegative safe integer in milliseconds; missing or zero means unlimited, while null, fractions and expressions are invalid. JSON remains version 1. The budget includes all nested children, timers, processors and samplers, but excludes result-channel backpressure. On expiry the engine cancels the active sample, leaves the entire controller, and emits one failed `ControllerTimeout` sample named after the controller (unnamed Loop: `Loop 循环`). Completed samples and variables survive; the interrupted sample is not counted twice. The earliest nested deadline wins, with the outer controller winning ties. `last_sample_ok` becomes false and the thread group error policy applies; Continue resumes after the controller. User cancellation and the group deadline retain their existing semantics.

Rust: `Node::LoopController(LoopController::new(name, count, children))` and `WhileController::timeout` use `Duration::ZERO` by default; legacy `Node::Loop` remains unlimited. `ComponentNode::LoopController` carries name/count/timeout/children; `ComponentNode::While` now also requires timeout. Direct cursor users handle `CursorStep::Timeout`, constrain pending sampler futures with `timeout_remaining()`, and exclude reporting waits with `exclude_reporting_time()`.
