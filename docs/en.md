# rmeter

**English** · [简体中文](crate::translations::zh_cn) · [繁體中文](crate::translations::zh_tw) · [日本語](crate::translations::ja) · [Français](crate::translations::fr) · [Deutsch](crate::translations::de)

The crate home page and API reference default to English. These guides are also available in five other languages; Rust identifiers and API reference comments remain English.

## Overview

`rmeter` is the script-driven entry point to this Rust/Tokio load-testing workspace. It parses version 1 JSON plans, compiles HTTP, PostgreSQL and JavaScript components, and streams run events. The default build is a library; the optional `cli` feature enables the command-line runner.

## Quick start

Run this example inside an existing Tokio runtime, with an HTTP service listening at the example URL. The event receiver must run concurrently: a full bounded channel pauses the engine. Add `rmeter` as a local Cargo path dependency, plus `tokio` and `serde_json` for this example.

```rust,no_run
use rmeter::{report::{ReportEvent, RunSummary}, run_script};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let source = r#"{
  "version": 1,
  "name": "Smoke test",
  "groups": [{
    "name": "Users",
    "users": 2,
    "iterations": 1,
    "children": [{
      "type": "http_request",
      "name": "Health",
      "url": "http://127.0.0.1:8080/health"
    }]
  }]
}"#;
let (sender, mut receiver) = tokio::sync::mpsc::channel(128);
let consume = async move {
    let mut summary = RunSummary::default();
    while let Some(event) = receiver.recv().await {
        summary.observe(&event);
        let report = ReportEvent::from(event);
        println!("{}", serde_json::to_string(&report).unwrap());
    }
    summary
};
let (result, summary) = tokio::join!(run_script(source, sender), consume);
result?;
assert_eq!(summary.failed, 0);
# Ok(())
# }
```

## Execution and errors

[`parse_script`](crate::parse_script) validates and compiles without sending HTTP requests or connecting to PostgreSQL; JavaScript and native HTTP extractors require the separate worker even during validation. UTF-8 JSON may begin with a BOM. Unknown fields and unsupported versions are rejected. User variables are strings and are isolated per virtual user.

[`run_script`](crate::run_script) and [`run_script_file`](crate::run_script_file) use the caller's runtime. Dropping the running future cancels virtual users. Failed samples are reported in events and may still leave the engine result as `Ok(())`; inspect `success`, [`RunSummary`](crate::report::RunSummary), and the returned result. `Finished` alone does not guarantee success. Event indices are zero-based; serialized timestamps are Unix milliseconds.

## API map

- [`script`](crate::script): parsing, file loading and [`ScriptError`](crate::ScriptError).
- [`report`](crate::report): serializable events and sample counters.
- [`core`](crate::core): plans, controllers, contexts and the engine.
- [`component`](crate::component): HTTP/JDBC/JavaScript samplers, assertions, extractors and timers.

## JavaScript and extractor worker

Build the worker from this workspace and deploy the matching executable beside the host application, or configure its trusted absolute path with `rmeter_component::javascript::set_worker_executable` before compiling plans (through `rmeter::component` when using the facade). `RMETER_JS_WORKER` is another host configuration option. JavaScript nodes, processors and native HTTP extractors need the worker. Plain HTTP/JDBC plans without those features do not. Worker isolation limits resources and contains failures; it is not a complete OS sandbox.

```sh
cargo build -p rmeter-component --release --bin rmeter-js-worker
```

## CLI and documentation

```sh
cargo run -p rmeter --features cli -- examples/http.json --check
cargo run -p rmeter --features cli -- examples/http.json
cargo doc --workspace --no-deps --open
cargo test --workspace --doc
```

## Native functions

The five functions RandomString, changeCase, dateTimeConvert, Random and RandomDate are shared by Rust, JavaScript (`functions`) and `${__function(...)}` expressions. Date patterns follow JDK 21 ISO/SMART rules using bundled locale and timezone data; runtime Java is not needed. Defaults use the system timezone and locale captured at run start. Invalid calls fail explicitly. Optional variable names store results for the current user; failed expressions roll back their writes. JavaScript source is never interpolated. Controller evaluation now takes a mutable UserContext to support function writes.
