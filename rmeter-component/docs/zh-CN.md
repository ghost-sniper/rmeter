# rmeter-component

[English](crate) · **简体中文** · [繁體中文](crate::translations::zh_tw) · [日本語](crate::translations::ja) · [Français](crate::translations::fr) · [Deutsch](crate::translations::de)

crate 首页和 API 参考默认使用英文。此使用指南提供六种语言；Rust 标识符及 API 参考注释保留英文。

## 概述

`rmeter-component` 为 `rmeter-core` 提供 HTTP/HTTPS、PostgreSQL（采用 JDBC 风格命名）和同步 JavaScript 采样器，还提供 JSONPath、正则及 XML 断言、HTTP 响应提取器、作用域默认值和常量/高斯定时器。PostgreSQL 使用 Rust 原生驱动，不加载 JVM 或 Java JDBC 驱动。

## 快速开始

在 Tokio runtime 内执行示例，本地服务应返回 `{"ready":true}`。构造函数校验静态配置；只有等待 `sample` 时才发送 HTTP 请求。

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

## 执行与错误处理

[`ComponentCompiler`](crate::ComponentCompiler) 将词法作用域默认值、命名数据库池和定时器编译为执行节点。内层默认值覆盖外层，请求显式设置覆盖继承值。请求显式 header/query 列表替换继承列表。兼容的 HTTP 客户端及命名数据库池可以共享，用户变量保持隔离。HTTP 不自动保存 Cookie。

处理顺序为前置处理器 → 定时器 → 采样器 → 后置处理器 → 断言。处理器不产生单独采样事件；处理器或断言失败会使父采样失败。JavaScript 仅在成功时提交变量修改；变量回滚不撤销外部数据库操作。后置处理器通过只读 `prev` 读取断言前的当前结果。

HTTP 报告保存、断言输入、后置输入分别限额。截断保存的响应体不会作为完整断言输入。HTTP 运行错误以失败的 `SampleResult` 返回，即使 `sample` 返回 `Ok` 也须检查 `success`。

## API 导航

- [`http`](crate::http)：请求、响应体、默认值和纯 HTTP 树编译。
- [`jdbc`](crate::jdbc)：命名 PostgreSQL 池及参数化查询。
- [`javascript`](crate::javascript)：脚本、限制和 worker 配置。
- [`assertion`](crate::assertion) / [`extractor`](crate::extractor)：响应检查与变量提取。
- [`component`](crate::component) / [`timer`](crate::timer)：混合树和延迟。

## JavaScript 与提取器 worker

从本工作区构建 worker，将同版本程序部署到宿主程序旁边，或在编译计划前通过 `rmeter_component::javascript::set_worker_executable` 配置可信绝对路径（使用统一入口时经由 `rmeter::component`）。也可由宿主设置 `RMETER_JS_WORKER`。JavaScript 节点、处理器和原生 HTTP 提取器需要 worker；不使用这些功能的纯 HTTP/JDBC 计划不需要。worker 提供资源与故障隔离，并非完整的操作系统沙箱。

```sh
cargo build -p rmeter-component --release --bin rmeter-js-worker
```
