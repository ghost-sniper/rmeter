# rmeter

[English](crate) · **简体中文** · [繁體中文](crate::translations::zh_tw) · [日本語](crate::translations::ja) · [Français](crate::translations::fr) · [Deutsch](crate::translations::de)

crate 首页和 API 参考默认使用英文。此使用指南提供六种语言；Rust 标识符及 API 参考注释保留英文。

## 概述

`rmeter` 是此 Rust/Tokio 压测工作区的脚本入口，负责解析版本 1 的 JSON 计划，编译 HTTP、PostgreSQL 和 JavaScript 组件，并输出运行事件。默认构建库；启用可选 `cli` feature 后可使用命令行程序。

## 快速开始

在已有 Tokio runtime 内运行下面的示例，并在示例地址启动 HTTP 服务。必须并发消费事件：有界通道填满后，引擎会等待。使用 Cargo 本地路径依赖添加 `rmeter`，并为本例添加 `tokio` 和 `serde_json`。

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

## 执行与错误处理

[`parse_script`](crate::parse_script) 校验并编译计划，不发送 HTTP 请求，也不连接 PostgreSQL；JavaScript 和原生 HTTP 提取器即使在校验时也需要独立 worker。支持带 BOM 的 UTF-8 JSON，拒绝未知字段和不支持的版本。用户变量为字符串，按虚拟用户隔离。

[`run_script`](crate::run_script) 和 [`run_script_file`](crate::run_script_file) 使用调用方的 runtime；丢弃运行 future 会取消虚拟用户。采样失败通过事件报告，引擎仍可能返回 `Ok(())`，因此须同时检查 `success`、[`RunSummary`](crate::report::RunSummary) 和返回结果。仅收到 `Finished` 不代表成功。事件索引从零开始，序列化时间戳为 Unix 毫秒。

## API 导航

- [`script`](crate::script)：解析、文件加载和 [`ScriptError`](crate::ScriptError)。
- [`report`](crate::report)：可序列化事件与采样计数。
- [`core`](crate::core)：计划、控制器、上下文和引擎。
- [`component`](crate::component)：HTTP/JDBC/JavaScript 采样器、断言、提取器和定时器。

## JavaScript 与提取器 worker

从本工作区构建 worker，将同版本程序部署到宿主程序旁边，或在编译计划前通过 `rmeter_component::javascript::set_worker_executable` 配置可信绝对路径（使用统一入口时经由 `rmeter::component`）。也可由宿主设置 `RMETER_JS_WORKER`。JavaScript 节点、处理器和原生 HTTP 提取器需要 worker；不使用这些功能的纯 HTTP/JDBC 计划不需要。worker 提供资源与故障隔离，并非完整的操作系统沙箱。

```sh
cargo build -p rmeter-component --release --bin rmeter-js-worker
```

## 命令行与文档

```sh
cargo run -p rmeter --features cli -- examples/http.json --check
cargo run -p rmeter --features cli -- examples/http.json
cargo doc --workspace --no-deps --open
cargo test --workspace --doc
```

## 原生函数

RandomString、changeCase、dateTimeConvert、Random、RandomDate 由 Rust、JavaScript 的 `functions` 对象和 `${__函数(...)}` 表达式共用。日期模式采用 JDK 21 ISO/SMART 规则，地区和时区数据内置，运行时不需要 Java。默认时区和语言在运行开始时从系统读取。非法调用明确报错；可选变量名将结果写入当前用户，表达式失败时回滚其写入。JavaScript 源码不进行表达式替换。控制器求值接口现接收可变 UserContext，以支持函数写变量。
