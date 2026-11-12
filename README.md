# rmeter

## Crate documentation

Run `cargo doc --workspace --no-deps --open` for the English API reference.
Each crate includes guides in 简体中文、繁體中文、日本語、Français and Deutsch
under **translations**. See [documentation build and language guide](docs/README.md).

基于 Rust/Tokio 的测试执行库与可选命令行程序。根包统一提供脚本解析、
HTTP/JDBC 组件编译、运行入口和可序列化事件。`rmeter-core` 负责调度与控制器，
`rmeter-component` 负责 HTTP、数据库连接、采样和响应断言。

完整的 IF、While、Loop 与 HTTP 采样测试见 [控制器集成示例](examples/controllers_http.md)。

## JavaScript 脚本变量

使用 `rquickjs` 执行同步 JavaScript，支持 ES6（ECMAScript 2015）语法。
引擎也接受其支持的较新语法。在 `children` 中添加节点：

```json
{
  "type": "javascript",
  "name": "生成变量",
  "timeout_ms": 100,
  "script": "const n = Number(vars.get('count') || '0') + 1; vars.put('count', String(n)); vars.put('requestId', `user-${ctx.userIndex}-${n}`);"
}
```

后续 HTTP/JDBC 请求可以使用 `${requestId}`、`${count}` 等现有变量替换。
无需网络的变量示例见 [javascript.json](examples/javascript.json)；同时验证 HTTP、JDBC、
IF 和 While 的步骤见 [JavaScript 变量联动示例](examples/javascript_pipeline.md)。
`--check` 会检查 JavaScript 语法和限制配置，但不会执行脚本。

脚本 API：

| API | 行为 |
| --- | --- |
| `vars.get(name)` | 读取字符串变量；不存在时返回 `null` |
| `vars.put(name, value)` | 新增或覆盖字符串变量；名称和值均须为字符串，数字使用 `String(value)`，对象使用 `JSON.stringify(value)` |
| `vars.remove(name)` | 删除变量；返回是否存在该变量 |
| `vars.contains(name)` | 判断变量是否存在；空字符串也算存在 |
| `ctx.groupName` / `ctx.userIndex` / `ctx.iteration` / `ctx.lastSampleOk` | 只读执行信息，用户和迭代索引从 0 开始 |

变量按虚拟用户隔离，在该用户后续节点和迭代间保留。每次执行使用新的
JavaScript 上下文，JS 全局变量不保留；只有脚本成功后才提交 `vars` 的修改，
抛异常、超过限制或取消时不提交。源码不会进行 `${...}` 替换，ES6 模板字符串可直接使用。
不运行 Promise 后续任务，不提供 Node.js、浏览器或异步 I/O API。
旧脚本的 `vars.set` 仍可使用；将 `vars.get(name) === undefined` 改为
`!vars.contains(name)` 或 `vars.get(name) === null`。

JavaScript 节点产生一条采样事件，参与采样数、成功率和耗时统计，更新
`last_sample_ok`，失败遵循线程组 `on_error` 策略。事件保留节点名称；
`response_code` 为 `OK`、`JavaScriptError`、`Timeout` 或基础设施错误 `Internal`，
失败详情见 `response_message`。

| 限制字段 | 默认值 | 含义 |
| --- | --- | --- |
| `timeout_ms` | 100 | 执行超时，允许 1–60000 毫秒 |
| `max_variable_bytes` | 1048576 | 所有变量名和值的 UTF-8 字节总量，包含初始变量 |

变量上限必须大于 0，脚本源码最大 1 MiB。请从旧脚本移除 `loop_iteration_limit`、
`recursion_limit`、`stack_size_limit`。QuickJS 的堆内存和原生栈上限分别为 64 MiB 和 256 KiB；
引擎通过中断回调检查执行超时。语法检查、编译和运行均在独立的
`rmeter-js-worker` 进程中执行，每次检查或执行使用新进程。父进程监督超时和取消，
如果引擎未及时退出，父进程会强制终止并回收子进程；worker 栈溢出或内存耗尽
只使本次检查/采样失败。超时或异常返回不提交变量，不回退到主进程执行。

每个宿主进程最多同时运行 4 个 worker，排队及启动阶段分别最多等待 5 秒。
worker 就绪后计时：语法检查/编译上限 2 秒，脚本执行使用 `timeout_ms`，包括输入
传输、解析、编译和执行。IPC 请求/响应各自最多 64 MiB。Windows 在发送脚本前
使用 Job Object 设置单 worker 256 MiB 提交内存限制、禁止派生子进程和关闭任务时
终止进程；Unix 使用 `RLIMIT_AS` 将地址空间限制为 256 MiB，设置 65 秒 CPU 时间
兜底上限并禁用 core dump，Linux 还设置父进程退出时终止 worker。
资源限制建立失败时拒绝执行。worker 不继承宿主环境变量，脚本及凭据不通过命令行传递。
这是资源与故障隔离；worker 仍以宿主用户身份运行，并非低权限账户或完整的系统调用沙箱。

### 部署 JavaScript worker

使用 JavaScript 节点、后置处理器或相应 `--check` 前，先构建 worker：

```sh
cargo build -p rmeter-component --release --bin rmeter-js-worker
```

将同版本的 `rmeter-js-worker.exe`（Unix 为 `rmeter-js-worker`）部署到宿主可执行文件
旁边。宿主程序也可以在解析测试计划前调用
`rmeter::component::javascript::set_worker_executable(绝对路径)`，或在启动前设置
`RMETER_JS_WORKER` 为可信 worker 的绝对路径。库和 Tauri 集成都需要部署该程序；
Tauri 可将其作为随应用分发的资源并通过上述 Rust API 指定路径。
worker 路径由宿主配置，测试计划不能覆盖；不从 PATH 或当前工作目录查找。
Cargo 开发目录还支持从当前测试程序所在的 `target/debug` 或 `target/release` 找到 worker。
缺少 worker、协议不匹配或启动失败会返回错误，HTTP/JDBC 不含脚本时无需 worker。

## JavaScript 前置与后置处理器

在 `http_request`、`jdbc_request` 或 `javascript` 节点中配置 `post_processors`，可用 JavaScript
读取当前采样器返回结果，并将提取值写入用户变量供后续请求使用：

```json
{
  "type": "http_request",
  "name": "登录",
  "url": "http://localhost:8080/login",
  "settings": {"save_response_body": false},
  "max_post_processor_body_bytes": 8388608,
  "post_processors": [
    {
      "type": "javascript",
      "name": "提取 token",
      "timeout_ms": 100,
      "script": "if (!prev.success || prev.body === null) throw new Error(prev.bodyUnavailableReason || 'login failed'); vars.put('token', JSON.parse(prev.body).token);"
    }
  ]
}
```

后续请求的 Header 可配置为 `[["Authorization", "Bearer ${token}"]]`。
完整 HTTP → JDBC → HTTP 示例见 [post_processors.json](examples/post_processors.json)，
运行前需准备登录接口和 PostgreSQL；`--check` 不连接这些服务，也不执行处理器。

后置脚本支持与独立 JavaScript 节点相同的执行限制、`vars` 和 `ctx` API。
另外注入只读 `prev`，它是所属采样器在断言检查前的结果快照：

| 属性 | 含义 |
| --- | --- |
| `prev.label` | 当前采样名称，已展开变量 |
| `prev.success` | 当前传输/查询/脚本是否成功（尚未执行断言） |
| `prev.responseCode` | HTTP 状态码或 JDBC 返回码；不可用时为 `null` |
| `prev.responseMessage` | 返回说明或错误信息；不可用时为 `null` |
| `prev.elapsedMs` | 采样耗时，单位毫秒，不包含后置处理时间 |
| `prev.body` | 完整 UTF-8 返回体文本，空响应为 `""`；不可用时为 `null` |
| `prev.bodyUnavailableReason` | 返回体不可用原因，正常时为 `null` |

`prev` 仅在后置处理器中存在，不保存在用户上下文，不会泄漏到下一次采样或独立
JavaScript 节点。`ctx.lastSampleOk` 仍表示引擎记录的上一次采样状态；
后置处理器应使用 `prev.success` 判断当前采样。完整返回体不自动写入 `vars`，
因此不占变量字节配额；通过 `vars.put` 保存的内容仍受配额限制。

JDBC 的 `prev.body` 是现有查询结果 JSON，包含 `columns`、`rows`、
`row_count` 和 `rows_affected`；每行数据按列顺序排列。例如：

```javascript
if (!prev.success || prev.body === null) throw new Error("查询失败");
const data = JSON.parse(prev.body);
const index = data.columns.indexOf("id");
if (index < 0 || data.rows.length === 0) throw new Error("查询未返回 id");
vars.put("userId", String(data.rows[0][index]));
```

HTTP 仅在存在后置处理器时额外收集完整输入，`max_post_processor_body_bytes`
默认 8 MiB，必须大于 0，与断言输入及报告保存限制分别计算。关闭
`save_response_body` 或降低 `max_saved_body_bytes` 不影响后置读取。
HTTP 输入超限、读取不完整或不是有效 UTF-8 时，提供 `body: null` 和原因，
不会将截断内容交给脚本；输入超限本身不额外标记失败，脚本可检查原因并抛错。
响应仍会继续消费，以保持连接复用和字节统计。JDBC 复用受 `max_result_bytes`
限制的完整查询结果，不受报告响应体保存开关影响。

执行顺序为：按数组顺序执行前置处理器 → 请求/查询/脚本 → 按数组顺序执行后置处理器 → 断言 → 发送父采样事件
并应用 `on_error`。失败的请求也执行后置处理器，但取消正在执行的采样会中止处理链。
每个处理器单独提交变量；失败或超限时回滚该处理器修改、保留之前成功处理器的
修改，并跳过剩余处理器。父采样标记为失败，保留原返回码、响应体和字节统计，
在 `response_message` 中追加处理器名称及错误。处理器成功不能恢复父采样的失败状态。
变量回滚不回滚已经执行的数据库操作。

后置处理器不产生额外采样事件，不增加请求采样数；父采样的 `elapsed` 保留请求/
查询/主脚本耗时，不包含前后置处理时间（前置失败时记录失败前置耗时）。处理器运行完成后，引擎再根据最终结果更新 `last_sample_ok`，因此
后续 IF/While 和线程组错误策略可以观察到处理器失败。

## 选择编译目标

默认不启用命令行依赖，生成 Rust 库：

```sh
cargo build -p rmeter --release --lib
cargo build -p rmeter-component --release --bin rmeter-js-worker
```

库产物为 `target/release/librmeter.rlib`。Tauri 的 Rust 后端直接通过 Cargo
路径依赖调用本库；无需复制 rlib，也无需创建额外 Tokio runtime。
该入口是 Rust API，不提供 C ABI 或可供 JavaScript 直接加载的动态库。

需要命令行程序时启用 `cli` feature：

```sh
cargo build -p rmeter --release --features cli --bin rmeter
cargo build -p rmeter-component --release --bin rmeter-js-worker
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
| `http_defaults` | `settings`：当前层 HTTP 默认配置；可选 `name` |
| `jdbc_connection_configuration` | `name`、`variable_name`、`url` 及连接池配置；不产生采样事件 |
| `jdbc_request` | `name`、`connection`、`sql`；可选查询类型、参数、结果限制和断言 |
| `sequence` | `children`：按顺序执行 |
| `loop` | `count`（非负整数或 `"infinite"`）、`children`；可选 `name` |
| `controller` | `name`、`children`：通用顺序控制器 |
| `if` | `name`、`condition`、`children`；可选 `evaluate_all`，默认 false |
| `while` | `name`、`condition`、`children` |

测试计划、线程组、所有子节点、全局 JDBC 配置、断言与前/后置处理器支持可选的
`"enabled": false`；缺省或 `true` 为启用，仅接受布尔值。禁用项在编译执行计划前
被过滤，不校验其执行配置、不初始化 worker/连接池、不产生采样或断言结果。
禁用控制器/线程组会跳过整个子树，禁用配置不参与继承；启用的 JDBC 采样器如果
只引用了禁用的连接配置，仍会报告缺少连接。禁用整个计划时产生一个无采样的完成结果，
计划的版本与名称仍需有效。旧脚本未设置此字段时行为保持不变。

根 `defaults` 与节点 `settings` 支持 `protocol`、`host`、`port`、`path`、
`headers`、`query`、`connect_timeout_ms`、`request_timeout_ms`、`follow_redirects`、
`max_redirects`、`save_response_body`、`max_saved_body_bytes`、`use_system_proxy`。
Headers/Query 是二元字符串数组，例如 `[["Accept", "application/json"]]`，
可保留重复键。超时字段缺省表示继承，显式 null 表示禁用，整数表示毫秒。
Defaults 作用于同层及后代；子作用域不泄漏。同层多个 Defaults 后者覆盖前者。
Loop 与 HTTP 默认配置的 `name` 用于脚本编辑器展示，不影响循环执行或配置作用域，
也不产生额外采样。为兼容旧脚本，允许缺省或 `null`；显式字符串名称不可为空白。
采样器显式设置的字段覆盖默认值，未设置的字段继续继承。采样器的
`headers`、`query` 一旦设置数组就整组替换默认列表（保留重复键和顺序），
`[]` 清空该列表；缺省或 `null` 继承。Defaults 组件之间仍按键合并列表。

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
JDBC 默认强制 TLS，验证系统信任的证书与主机名，服务器拒绝 TLS 时直接失败，
不会降级后发送用户名或密码。`sslmode=prefer` 也按强制 TLS 处理。
仅在可信本地开发环境需要明文时显式设置 `sslmode=disable`。
完整示例为 [jdbc_postgresql.json](examples/jdbc_postgresql.json)，包含配置、建表、参数化写入及 IF + Loop 查询：

```sh
cargo run -p rmeter --features cli -- examples/jdbc_postgresql.json --check
cargo run -p rmeter --features cli -- examples/jdbc_postgresql.json --var db_url=postgresql://user@localhost:5432/testdb?sslmode=disable
```

示例预期 4 次采样、5 项断言通过；`--check` 不连接数据库，不检查 SQL 语法或权限。

## 验证

```sh
cargo build -p rmeter-component --bin rmeter-js-worker
cargo test --workspace
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
```


### 前置处理器与日志 API

三个采样器的 `pre_processors` 与 `post_processors` 均为数组，元素格式相同：
`{"type":"javascript","name":"准备参数","script":"vars.put('id','42'); log.info('准备完成');","timeout_ms":1000}`。
可设置 `enabled:false`，处理器不允许再挂载子处理器。前置在模板展开前运行，成功写入的变量立即供本次采样器使用；前置失败跳过采样器、后置与断言，将父采样标为失败并应用线程组错误策略。每个处理器成功后提交其变量，失败回滚自身变量，同阶段余下处理器不再执行。取消会回收执行中的 worker。

前后置以及独立 JavaScript 采样器均提供 `vars.get/put/set/remove/contains/getIteration`（字符串变量，getIteration 从 1 开始）、`ctx.groupName/userIndex/iteration/lastSampleOk`、`ctx.getThreadNum()`、`ctx.getVariables()`、只读 `sampler.name/getName()` 和处理器/采样器名称 `Label`。`ctx.iteration`、`userIndex` 从 0 开始。
`log.trace/debug/info/warn/error(message, ...args)` 支持 `{}` 占位符，`log.isDebugEnabled/isInfoEnabled()` 返回 true。日志随父采样的 `script_logs.entries` 输出，带 level/source/message；单条最多 4096 UTF-8 字节，每次采样累计最多 100 条、64 KiB，超限设置 `script_logs.truncated`。脚本异常仍返回已收集日志；worker 被外部取消或硬超时终止时，未返回的日志丢弃。日志不增加采样数，也不写入变量。

后置的只读 `prev` 新增 `getResponseDataAsString/getResponseCode/getResponseMessage/getSampleLabel/getTime/isSuccessful` 方法，对应已有字段。前置及独立采样器不提供 `prev`；JavaScript 采样器的后置提供本次执行状态，但无响应体（body 为 null）。这些 API 参考 JMeter 的 JSR223 使用方式，不提供 Java 对象、props、文件或网络 I/O。

兼容变化：后置改在断言之前执行，因此 prev.success 只反映采样器状态；失败断言仍使最终采样失败。采样事件 RunEvent::SampleCompleted 的载荷改为 Box<SampleEvent>，包含有界日志；JSON 报告结构仅新增 script_logs。


## HTTP 响应提取器

HTTP `post_processors` 现在支持混合排列 `javascript` 与 `jsonpath_extractor`、`xpath_extractor`、`jmespath_extractor`、`regex_extractor`、`xpath2_extractor`。只允许在 HTTP 后置阶段使用提取器，前置、JDBC、JavaScript 采样器不接受它们。禁用与其他组件相同（`enabled:false`）。

```json
{"type":"jsonpath_extractor","name":"提取 token","variable_name":"token","expression":"$.token","match_number":1,"default_value":"","timeout_ms":1000,"max_matches":1000,"max_variable_bytes":1048576}
```

共同字段为 name、variable_name、expression、match_number（默认 1；0 随机，-1 全部）、default_value（省略/null 不设置基础变量，空字符串可显式设置）、timeout_ms（1–60000，默认 1000）、max_matches（1–100000，默认 1000）、max_variable_bytes（默认 1 MiB，含用户已有变量）。输出为字符串：字符串原样保存，JSON 数字/对象/数组/null 用 JSON 文本。JMESPath 顶层数组分为多个匹配，null 表示无匹配；JSONPath 中明确选中的 null 保存为 "null"。

每次执行先清除该变量的旧基础值、编号值和正则分组，初始化 `变量_matchNr=0` 及配置的默认值。正常完成后 `变量_matchNr` 是匹配总数；正数或随机模式将选中项写入基础变量；全部模式写入 `变量_1`、`变量_2`……，基础变量维持默认值。未匹配或第 N 项越界正常使用默认值。解析错误、容量/匹配数量超限或超时令父采样失败，并停止后续后置处理器；不保留旧值或部分新值。取消时也不会提交提取结果。

正则使用 Rust regex（不支持环视/回溯引用），template 默认 `$1$`，支持 `$0$` 整体匹配及 `$N$` 捕获组，例如 `ID=$1$`。分组存入 `变量_g0`、`变量_g1`……，`变量_g` 是捕获组数；全部模式对应 `变量_1_g0` 等。不存在的分组模板在校验阶段报错。

XPath 使用 SXD XPath 1.0；XPath2 使用 Xee XPath 3.1 引擎，支持 XPath 2.0 的序列、for、upper-case、string-join 等。两者支持 namespaces 字符串映射；XPath2 支持空前缀指定默认元素命名空间。节点返回字符串值，标量/序列按值输出。XML 要求合法文档，不做 HTML 容错，不允许 DTD/外部实体；XPath2 默认动态上下文不注册外部资源或进程环境变量。

五种提取器都在同版本 `rmeter-js-worker` 中解析/执行，复用进程内存、执行超时和取消回收机制，避免复杂 XPath2 表达式阻塞宿主。worker 缺失时带提取器的计划会明确校验失败；不使用 JavaScript/提取器的 HTTP/JDBC 仍可运行。提取输入使用完整 UTF-8 HTTP 响应，与报告预览保存无关，受 `max_post_processor_body_bytes` 限制（默认 8 MiB）。输入不可用时失败，不对截断正文做部分提取。处理器只增加诊断日志，不额外增加采样数量。

Rust API：HTTP 的 `post_processors` 改为 `Vec<HttpPostProcessorConfig>`；`add_post_processor` 接受 JavaScriptConfig 或 ExtractorConfig，也可以将它们 `.into()` 放入数组。原生 JSON 中现有 JavaScript 后置处理器格式不变。

## 定时器

固定定时器和高斯随机定时器采用 [JMeter 的作用域规则](https://jmeter.apache.org/usermanual/component_reference.html#Timers)。测试计划的 `timers` 影响所有线程组；线程组/控制器 `children` 中的定时器影响同级及后代采样器，与同级排列位置无关；采样器自身 `timers` 只影响自身。所有适用定时器的延迟累加，每次采样、每轮循环和每个用户分别执行，空作用域及未进入的条件分支不等待。

```json
{"type":"constant_timer","name":"固定等待","delay_ms":300}
```

```json
{"type":"gaussian_random_timer","name":"高斯等待","deviation_ms":100,"offset_ms":300}
```

高斯延迟与 JMeter 实现一致：`abs(N(0,1) * deviation_ms + offset_ms)` 后截去小数，每次独立抽样。参数必须是 0–9007199254740991 的有限非负数；延迟/偏移是整数，偏差允许小数。计算结果也以此上限截断。参数支持字符串 `${pause_ms}`，在前置处理器之后读取当前用户变量；不解释 JMeter 函数或 `timer.factor` 属性。

执行顺序：前置处理器 → 计算并累加延迟 → 异步等待 → 采样器 → 后置处理器 → 断言。等待不计入采样开始时间和响应耗时，但计入线程组最长运行时间；停止或关闭通道可取消等待，不发请求、不产生额外采样。动态参数错误使父采样失败（`TimerError`），跳过后续阶段并遵循错误策略。禁用定时器在校验前跳过。仅使用定时器的 HTTP/JDBC 计划不需要 JavaScript worker。
