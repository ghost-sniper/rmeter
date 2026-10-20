# rmeter

基于 Rust/Tokio 的测试执行库与可选命令行程序。根包统一提供脚本解析、
HTTP/JDBC 组件编译、运行入口和可序列化事件。`rmeter-core` 负责调度与控制器，
`rmeter-component` 负责 HTTP、数据库连接、采样和响应断言。

完整的 IF、While、Loop 与 HTTP 采样测试见 [控制器集成示例](examples/controllers_http.md)。

## 选择编译目标

默认不启用命令行依赖，生成 Rust 库：

```sh
cargo build -p rmeter --release --lib
```

库产物为 `target/release/librmeter.rlib`。Tauri 的 Rust 后端直接通过 Cargo
路径依赖调用本库；无需复制 rlib，也无需创建额外 Tokio runtime。
该入口是 Rust API，不提供 C ABI 或可供 JavaScript 直接加载的动态库。

需要命令行程序时启用 `cli` feature：

```sh
cargo build -p rmeter --release --features cli --bin rmeter
cargo run -p rmeter --features cli -- examples/http.json
cargo run -p rmeter --features cli -- examples/http.json --check
cargo run -p rmeter --features cli -- examples/http.json --var host=localhost --var port=8080
```

Windows 可执行文件为 `target/release/rmeter.exe`；其他平台为 `target/release/rmeter`。
`--check` 解析并编译配置，不发送 HTTP 请求。变量在采样/条件求值时展开，
因此检查模式不会保证运行时变量完整或服务器可访问。
重复 `--var KEY=VALUE` 可覆盖脚本变量，同名变量以最后一次为准。

运行时标准输出为 JSON Lines：`started`、`sample_completed`、`finished`，
最后输出 `summary`，可重定向保存；错误说明写入标准错误。
事件包含 HTTP 状态、断言诊断、用户/迭代索引、耗时和字节统计。
索引零起始，时间戳是 Unix 毫秒；保存的响应体以字节数组表示。

退出码：`0` 执行成功且全部采样通过（或检查通过），`1` 至少一项采样失败，
`2` 参数、文件、解析、配置或引擎错误，`130` 用户按 Ctrl+C 取消。
取消会中止正在运行的采样与虚拟用户，不补发 `finished`。
`finished` 表示任务收尾完成，仍需检查引擎返回值或 summary 的 `error`。

## 库调用与 Tauri 接入

在后续 Tauri 项目的 `src-tauri/Cargo.toml` 中添加：

```toml
[dependencies]
rmeter = { path = "../../rmeter", default-features = false }
tokio = { version = "1", features = ["sync"] }
```

路径应按实际项目目录调整。库不依赖 Tauri，可由异步 command 或后台任务调用：

`ReportEvent` 实现 `Serialize` 与 `Clone`，满足 Tauri
[Emitter::emit](https://docs.rs/tauri/latest/tauri/trait.Emitter.html#method.emit)
的事件数据要求，可在接收端直接转发。

```rust,no_run
use rmeter::{run_script_file, report::ReportEvent};

# async fn example() -> Result<(), rmeter::ScriptError> {
let (sender, mut receiver) = tokio::sync::mpsc::channel(256);
let (result, ()) = tokio::join!(
    run_script_file("examples/http.json", sender),
    async {
        while let Some(event) = receiver.recv().await {
            let report = ReportEvent::from(event);
            // Forward the serializable report to a Tauri event or channel.
            // With a Tauri AppHandle: app.emit("rmeter:event", report)?;
            println!("{}", serde_json::to_string(&report).unwrap());
        }
    },
);
result?;
# Ok(())
# }
```

`parse_script(&str)` / `load_script(path).await` 返回 `TestPlan<ComponentSampler>`，
可修改计划变量后调用 `Engine::new(plan).run(sender).await`。
`run_script` / `run_script_file` 提供直接执行入口。
必须并发消费有界事件通道，否则运行可能因通道已满而等待。
关闭事件接收端或丢弃运行 Future 会取消任务；前端可基于此接入停止操作。
库不创建 runtime，也不将事件或测试结果写入标准输出。
`ComponentSampler` 可混合 HTTP 和 JDBC，使用 `as_http()` / `as_jdbc()` 访问具体组件。
原有 `HttpNode` / `compile_http_nodes` 继续支持纯 HTTP Rust 计划。

## JSON 脚本格式（version 1）

仅支持 UTF-8 JSON（允许 UTF-8 BOM），不支持 JMeter JMX/XML、YAML 或脚本语言。
未知字段会报错，避免字段拼写错误被忽略。最小 HTTP 示例：

```json
{
  "version": 1,
  "name": "HTTP 业务检查",
  "variables": {"host": "127.0.0.1", "port": "8080", "enabled": "true"},
  "defaults": {"protocol": "http", "host": "${host}", "request_timeout_ms": 5000},
  "groups": [{
    "name": "HTTP 用户",
    "users": 1,
    "ramp_up_ms": 0,
    "iterations": 1,
    "on_error": "continue",
    "children": [{
      "type": "http_request",
      "name": "查询状态",
      "url": "http://${host}:${port}/status",
      "assertions": [{
        "name": "业务状态成功",
        "path": "$.code",
        "matcher": {"type": "equals", "expected": 0},
        "selection": "one"
      }]
    }]
  }]
}
```

计划必须提供 `version`、`name` 和 `groups`。变量值必须是字符串。
组必须提供 `name` 和 `children`；默认 `users=1`、`ramp_up_ms=0`、`iterations=1`、
`on_error=continue`，`max_duration_ms` 缺省或 null 表示无限时。
`users=0` 禁用该组；`iterations=0` 跳过执行。无限迭代写作 `"infinite"`。
错误策略可选 `continue`、`next_iteration`、`stop_user`、`stop_test`。
计划、线程组、请求、控制器和断言名称不得为空。

子节点的 `type`：

| type | 配置 |
| --- | --- |
| `http_request` | `name`；可选 `url`、`method`（默认 GET）、`settings`、`body`、`assertions`、`max_assertion_body_bytes` |
| `http_defaults` | `settings`：当前层 HTTP 默认配置 |
| `jdbc_connection_configuration` | `name`、`variable_name`、`url` 及连接池配置；不产生采样事件 |
| `jdbc_request` | `name`、`connection`、`sql`；可选查询类型、参数、结果限制和断言 |
| `sequence` | `children`：按顺序执行 |
| `loop` | `count`（非负整数或 `"infinite"`）、`children` |
| `controller` | `name`、`children`：通用顺序控制器 |
| `if` | `name`、`condition`、`children`；可选 `evaluate_all`，默认 false |
| `while` | `name`、`condition`、`children` |

根 `defaults` 与节点 `settings` 支持 `protocol`、`host`、`port`、`path`、
`headers`、`query`、`connect_timeout_ms`、`request_timeout_ms`、`follow_redirects`、
`max_redirects`、`save_response_body`、`max_saved_body_bytes`、`use_system_proxy`。
Headers/Query 是二元字符串数组，例如 `[["Accept", "application/json"]]`，
可保留重复键。超时字段缺省表示继承，显式 null 表示禁用，整数表示毫秒。
Defaults 作用于同层及后代；子作用域不泄漏。同层多个 Defaults 后者覆盖前者。

请求 `body` 默认空，可写为：

```json
{"type": "json", "value": {"user": "${user}"}}
```

其他类型：`empty` 无 value；`text` 为字符串；`bytes` 为 0–255 整数数组；
`form` 为二元字符串数组。JSON 仅在字符串值中展开变量，bytes 不展开。

HTTP 请求的 `assertions` 可混合三种独立组件，按声明顺序执行并分别报告结果。
JSON 组件使用 `"type":"json"`；未写 type 的旧格式仍按 JSON 处理。
JSON 断言必须提供 `name`、`path`、`matcher`。matcher 支持：
`{"type":"equals","expected":0}`、`{"type":"regex","pattern":"^ok$"}`、
`{"type":"exists"}`、`{"type":"not_exists"}`。
selection 可选 `one`（默认）、`any`、`all`；相等和正则对空结果均失败，
存在检查区分缺失与 null。路径、正则和预期值不展开变量。

正文正则组件不依赖 JSON 解析，可直接检查文本、JSON、XML 等完整 UTF-8 响应：

```json
{"type":"regex","name":"无错误标记","pattern":"ERROR","invert":true}
```

省略 `invert` 时要求匹配；设置 true 时要求不匹配。`(?m)` 支持多行锚点，
`(?s)` 支持点匹配换行。匹配使用搜索语义，整串匹配可加 `^...$`。

XML 组件使用 XPath 选择节点后进行字符串相等、正则或存在检查：

```json
{
  "type": "xml",
  "name": "XML 业务码",
  "xpath": "/s:response/s:code/text()",
  "namespaces": {"s": "urn:status"},
  "matcher": {"type": "equals", "expected": "0"},
  "selection": "one"
}
```

`namespaces` 可省略；查询带默认命名空间的 XML 时也需显式绑定前缀。
XML 预期值是字符串，selection 和 matcher 的其他选项与 JSON 类似。
XPath 标量会按字符串值比较；存在检查仅用于节点集合。
当前 XML 要求 UTF-8，不支持 DOCTYPE/DTD、XSD 或非 UTF-8 编码转换。
三种组件均使用完整响应，独立缓存上限默认 8 MiB，任意失败将采样标记为失败。

运行三种组件的示例：在一个终端执行 `cargo run -p rmeter --example controller_http_server`，
在另一个终端执行 `cargo run -p rmeter --features cli -- examples/assertions.json`。
默认访问 `127.0.0.1:8080`，预期 3 次采样、6 项断言全部通过。
完整语义见 [组件文档](rmeter-component/README.md)。

IF/While 的 condition 可为布尔值、变量表达式字符串或结构化条件：

```json
{
  "type": "if",
  "name": "前两轮执行",
  "condition": {
    "type": "compare",
    "left": {"type": "iteration"},
    "operator": "less",
    "right": {"type": "literal", "value": "2"}
  },
  "children": []
}
```

上述条件检查线程组的零起始迭代索引。IF 字符串仅展开为 true 时进入；
While 字符串支持空条件、`LAST`，其他值仅展开为 false 时退出。
不执行 JavaScript/Groovy，也不支持 JMeter 函数。
结构化条件支持 `constant(value)`、`variable_bool(name)`、`last_sample_ok`、
`expression(value)`、`compare(left, operator, right)`、`not(condition)`、
`all(conditions)`、`any(conditions)`。
比较运算包括 `equal`、`not_equal`、`less`、`less_or_equal`、`greater`、`greater_or_equal`；
操作数支持 `literal(value)`、`variable(name)`、`iteration`、`user_index`、`last_sample_ok`。
详见 [核心文档](rmeter-core/README.md)。

## JDBC / PostgreSQL

JDBC 使用 Rust 原生 PostgreSQL 驱动，不依赖 JVM 或 JDBC JAR。
连接信息必须由 JDBC Connection Configuration 组件提供，采样器只引用其 `variable_name`。
连接配置支持作用域及计划顶层的 `jdbc_connections` 数组；详见
[JDBC 配置与使用](rmeter-component/src/jdbc/README.md)。
完整示例为 [jdbc_postgresql.json](examples/jdbc_postgresql.json)，包含配置、建表、参数化写入及 IF + Loop 查询：

```sh
cargo run -p rmeter --features cli -- examples/jdbc_postgresql.json --check
cargo run -p rmeter --features cli -- examples/jdbc_postgresql.json --var db_url=postgresql://user@localhost:5432/testdb?sslmode=disable
```

示例预期 4 次采样、5 项断言通过；`--check` 不连接数据库，不检查 SQL 语法或权限。

## 验证

```sh
cargo test --workspace
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
```
