# rmeter-component

**English** · [简体中文](crate::translations::zh_cn) · [繁體中文](crate::translations::zh_tw) · [日本語](crate::translations::ja) · [Français](crate::translations::fr) · [Deutsch](crate::translations::de)

The crate home page and API reference default to English. These guides are also available in five other languages; Rust identifiers and API reference comments remain English.

## Overview

`rmeter-component` supplies HTTP/HTTPS, PostgreSQL (using JDBC-style component names) and synchronous JavaScript samplers for `rmeter-core`. It also provides JSONPath, regex and XML assertions, HTTP response extractors, scoped defaults, and constant/Gaussian timers. PostgreSQL uses native Rust drivers; no JVM or Java JDBC driver is loaded.

## Quick start

Execute the example in a Tokio runtime against a local service returning `{"ready":true}`. Constructors validate static configuration; HTTP requests are sent only when `sample` is awaited.

```rust,no_run
use rmeter_component::{HttpRequestConfig, HttpSampler, JsonAssertionConfig};
use rmeter_core::{context::UserContext, sampler::Sampler};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let mut config = HttpRequestConfig::with_url(
    "Health", "http://127.0.0.1:8080/health",
);
config.add_assertion(JsonAssertionConfig::equals(
    "Ready", "$.ready", serde_json::json!(true),
));
let sampler = HttpSampler::new(config)?;
let mut user = UserContext::new("Users", 0);
let result = sampler.sample(&mut user).await
    .map_err(|error| format!("{error:?}"))?;
assert!(result.success, "{:?}", result.response_message);
# Ok(())
# }
```

## Execution and errors

[`ComponentCompiler`](crate::ComponentCompiler) resolves lexical defaults, named database pools and timers into execution nodes. Inner defaults override outer defaults; request settings override inherited defaults. Explicit request header/query lists replace inherited lists. Compatible HTTP clients and named database pools are shared, while user variables remain isolated. HTTP cookies are not stored automatically.

The pipeline is preprocessors → timers → sampler → postprocessors → assertions. Processors do not emit separate sample events. A failed processor or assertion marks its parent sample as failed. JavaScript variable changes commit only on successful execution; rollback does not undo external database operations. Postprocessors read the current result through readonly `prev` before assertions run.

HTTP reporting, assertion input and postprocessor input have separate body limits. A truncated saved body is not used as complete assertion input. Operational HTTP failures are unsuccessful `SampleResult` values, so inspect `success` even when `sample` returns `Ok`.

## API map

- [`http`](crate::http): requests, bodies, defaults and HTTP-only tree compilation.
- [`jdbc`](crate::jdbc): named PostgreSQL pools and parameterized queries.
- [`javascript`](crate::javascript): scripts, limits and worker configuration.
- [`assertion`](crate::assertion) / [`extractor`](crate::extractor): response checks and variables.
- [`component`](crate::component) / [`timer`](crate::timer): mixed trees and delays.

## JavaScript and extractor worker

Build the worker from this workspace and deploy the matching executable beside the host application, or configure its trusted absolute path with `rmeter_component::javascript::set_worker_executable` before compiling plans (through `rmeter::component` when using the facade). `RMETER_JS_WORKER` is another host configuration option. JavaScript nodes, processors and native HTTP extractors need the worker. Plain HTTP/JDBC plans without those features do not. Worker isolation limits resources and contains failures; it is not a complete OS sandbox.

```sh
cargo build -p rmeter-component --release --bin rmeter-js-worker
```

## Native functions

The five functions RandomString, changeCase, dateTimeConvert, Random and RandomDate are shared by Rust, JavaScript (`functions`) and `${__function(...)}` expressions. Date patterns follow JDK 21 ISO/SMART rules using bundled locale and timezone data; runtime Java is not needed. Defaults use the system timezone and locale captured at run start. Invalid calls fail explicitly. Optional variable names store results for the current user; failed expressions roll back their writes. JavaScript source is never interpolated. Controller evaluation now takes a mutable UserContext to support function writes.
