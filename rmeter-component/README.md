# rmeter-component

HTTP/HTTPS 采样器、HTTP Request Defaults，以及 JSON、正文正则和 XML 三种独立断言组件。
依赖 `rmeter-core`，可直接交给现有 `Engine` 执行。
还提供使用 Rust 原生 PostgreSQL 驱动的 JDBC 采样器与命名连接配置组件，
详见 [JDBC 使用文档](src/jdbc/README.md)。
还提供 Boa 同步 JavaScript 采样器 `JavaScriptConfig` / `JavaScriptSampler`，
通过 `vars.get/set/remove` 操作当前用户变量，支持执行限制和失败回滚；
配置与示例见 [JavaScript 脚本变量](../README.md#javascript-脚本变量)。
HTTP/JDBC 配置的 `post_processors: Vec<JavaScriptConfig>` 可挂载后置处理器，
通过只读 `prev` 提取当前返回结果；见 [后置处理器说明](../README.md#http--jdbc-后置处理器)。

## 创建采样器

`HttpSampler` / `HTTPSampler` 是同一类型；`HttpRequestDefaults` / `HTTPRequestDefaults` 也是同一类型。
Defaults 只提供配置，不发送请求，不产生采样事件。

```rust,no_run
use std::time::Duration;
use rmeter_component::{HttpRequestConfig, HttpRequestDefaults, HttpSampler, Setting};
use rmeter_core::{context::UserContext, sampler::Sampler};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let defaults = HttpRequestDefaults {
    protocol: Some("https".into()),
    host: Some("example.test".into()),
    request_timeout: Setting::Value(Some(Duration::from_secs(5))),
    ..Default::default()
};
let mut request = HttpRequestConfig::new("查询用户 ${id}");
request.settings.path = Some("/users/${id}".into());
request.settings.query = Some(vec![("search".into(), "${keyword}".into())]);
request.settings.save_response_body = Some(true);
let sampler = HttpSampler::with_defaults(request, &defaults)?;

let mut user = UserContext::new("HTTP 用户", 0);
user.variables.insert("id".into(), "42".into());
user.variables.insert("keyword".into(), "中文 & text".into());
// Variables expand for this user on every call; query/form values are encoded.
let result = sampler.sample(&mut user).await.unwrap();
println!("{}: {:?}", result.label, result.response_code);
# Ok(())
# }
```

`HttpRequestConfig::with_url(label, url)` 使用绝对地址，覆盖 protocol、host、port、path。
`method` 默认为 `HttpMethod::GET`。`HttpBody` 支持 Empty、Text、Bytes、Form 和 Json。
Text 和 Form 支持变量；Json 在字符串值中展开变量后序列化，保持正确的 JSON 转义。
Bytes 不展开变量。变量展开是单次替换，不递归展开变量值；缺失变量产生失败采样。

## 配置树与 Defaults 作用域

```rust,no_run
use rmeter_component::{HttpNode, HttpRequestConfig, HttpRequestDefaults, compile_http_nodes};
use rmeter_core::plan::{LoopCount, Node};

let nodes: Vec<Node<rmeter_component::HttpSampler>> = compile_http_nodes(
    vec![
        HttpNode::Defaults(HttpRequestDefaults {
            host: Some("example.test".into()),
            ..Default::default()
        }),
        HttpNode::Loop {
            count: LoopCount::Finite(2),
            children: vec![HttpNode::Request(HttpRequestConfig::new("首页"))],
        },
    ],
    &HttpRequestDefaults::default(),
).unwrap();
// Assign nodes to ThreadGroup.children, then run Engine::new(plan).run(sender).
```

- 合并优先级：请求显式配置 > 当前作用域 Defaults > 外层 Defaults > 内置值。
- Defaults 作用于同层请求及其子作用域，不受放置在请求之前或之后的影响。
- 同一作用域多个 Defaults 按声明顺序合并，后声明的显式字段覆盖前者。
- Sequence 和 Loop 建立子作用域，子配置不影响父作用域或其他同级作用域。
- 默认路径是完整缺省路径；显式请求路径不会拼接默认路径。路径可以携带原始查询字符串，结构化 query 参数另外编码追加。
- Headers 按名称忽略大小写覆盖；Query 按名称精确覆盖。保留未覆盖的继承项及同层重复键。`Some(vec![])` 清除继承列表。
- `Setting::Inherit` 继承超时，`Setting::Value(None)` 明确禁用，`Setting::Value(Some(duration))` 设置超时。
- 兼容传输配置的请求在一次树编译中共享客户端和连接池。

## 运行和结果约定

- 默认协议 HTTP，默认路径 `/`，连接超时 10 秒，整个请求超时 30 秒。
- 默认跟随最多 10 次重定向；重试关闭。整个请求超时覆盖响应体读取。
- HTTPS 使用系统 TLS 后端及证书验证。默认直连，`use_system_proxy: Some(true)` 可启用环境/系统代理。
- 无 Cookie 自动存储或用户会话功能；共享客户端不会共享登录 Cookie。
- 2xx/3xx 为成功状态；4xx/5xx 为失败状态，保留 HTTP 状态码和响应信息。
- 网络错误、超时和运行时变量/配置错误返回 `Ok` 中的失败 `SampleResult`，保留请求名称。引擎仍据 `success` 执行错误策略。
- 静态配置错误在构造/编译时返回 `HttpConfigError`；变量值相关的检查在采样时进行。
- 耗时包含请求构造、重定向及最终响应体完整读取。采样 Future 可以直接被引擎取消，不额外启动后台采样任务。
- `bytes_sent` 是初始请求体长度；`bytes_received` 是实际读取的最终响应体字节数。均不包含 Headers、TLS 或中间重定向流量。
- 默认不保存响应体，但仍完整消费并统计。启用保存后默认最多保留 1 MiB 前缀，`max_saved_body_bytes` 可调整；截断保存不截断读取或统计。
- 响应体读取出错时，失败结果保留已获得的状态码、字节数和已保存内容。

可运行示例：`cargo run -p rmeter-component --example http_request -- http://127.0.0.1:8080/`。

## JSON 断言

将断言配置添加到请求的 `assertions` 列表。采样器在读取完整响应后执行所有断言，
再返回结果供引擎更新 `last_sample_ok`、发送事件和执行错误策略。

```rust,no_run
use rmeter_component::{HttpRequestConfig, HttpSampler, JsonAssertionConfig};
use rmeter_core::{context::UserContext, sampler::Sampler};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let mut request = HttpRequestConfig::with_url("查询状态", "http://127.0.0.1:8080/status");
request.add_assertion(JsonAssertionConfig::equals(
    "业务状态成功", "$.code", serde_json::json!(0),
));
request.add_assertion(JsonAssertionConfig::equals(
    "用户已启用", "$.data.enabled", serde_json::json!(true),
));
let sampler = HttpSampler::new(request)?;
let mut user = UserContext::new("用户组", 0);
let result = sampler.sample(&mut user).await.unwrap();
for assertion in &result.assertions {
    println!("{}: success={}, {:?}", assertion.name, assertion.success, assertion.message);
}
# Ok(())
# }
```

匹配规则通过 `JsonMatcher` 指定：

| 匹配规则 | 含义 |
| --- | --- |
| `Equals(Value)` | 使用 `serde_json::Value` 相等比较，保留 JSON 类型；字符串 `"0"` 与数字 `0` 不相等。支持数组和对象比较。 |
| `Regex(String)` | 仅匹配 JSON 字符串节点，默认搜索匹配；整串匹配使用 `^...$`。 |
| `Exists` | JSONPath 至少选中一个节点；值为 `null` 仍算存在。 |
| `NotExists` | JSONPath 未选中任何节点。 |

相等和正则匹配通过 `MatchSelection` 指定多值语义：`One` 默认要求恰好一个值，
`Any` 要求至少一个值匹配，`All` 要求非空且所有值匹配。无结果时都失败，
避免将缺失字段当作 JSON null，或在空集合上错误通过断言。存在检查忽略该选项。

```rust
use rmeter_component::{JsonAssertion, JsonAssertionConfig, JsonMatcher, MatchSelection};

let assertion = JsonAssertion::new(JsonAssertionConfig {
    name: "所有用户名称合法".into(),
    path: "$.users[*].name".into(),
    matcher: JsonMatcher::Regex("^[A-Za-z]+$".into()),
    selection: MatchSelection::All,
}).unwrap();
assert!(assertion.evaluate_json(&serde_json::json!({
    "users": [{"name": "Alice"}, {"name": "Bob"}],
})).success);
```

- JSONPath 和正则在构造采样器时编译，非法配置返回 `HttpConfigError`；同一响应只解析一次 JSON。
- 每项结果保存在 `SampleResult.assertions`，包含名称、是否通过和失败信息；任意失败将采样 `success` 置为 false，保留 HTTP 状态码和传输信息。断言通过也不会将 HTTP 失败改为成功。
- 断言使用独立的完整响应体，不受 `save_response_body` 和报告截断影响。默认最多缓存 8 MiB，通过请求的 `max_assertion_body_bytes` 调整。
- 超过断言缓存限制、读取不完整、请求失败或 JSON 非法均导致断言失败，包括 `NotExists`。超过限制仍继续读取响应并统计字节。
- 未配置断言时不创建断言响应缓存、不解析 JSON。采样 `elapsed` 不包含断言求值耗时。
- 当前 JSONPath、预期值和正则为静态配置，不展开变量；断言按单个请求配置。

## 独立断言组件

`HttpRequestConfig.assertions` 现在是 `Vec<AssertionConfig>`，可以混合
`Json`、`Regex`、`Xml` 三种组件。各组件分别实现核心 `Assertion` 接口，
也可通过 `ResponseAssertion::new` 统一编译和独立求值。
原来直接对列表 push JSON 配置的 Rust 调用需添加 `.into()`，
或改用接受三种配置的 `request.add_assertion(...)`。

```rust,no_run
use rmeter_component::{HttpRequestConfig, HttpSampler, RegexAssertionConfig, XmlAssertionConfig};

let mut request = HttpRequestConfig::with_url("XML 状态", "http://127.0.0.1:8080/xml");
request.add_assertion(RegexAssertionConfig::new("响应包含状态", "<code>0</code>"));
request.add_assertion(XmlAssertionConfig::equals("XML 业务码", "/response/code/text()", "0"));
let sampler = HttpSampler::new(request).unwrap();
```

- **JSON**：保留上述 JSONPath、类型相等、字符串正则和存在检查语义。
- **正文正则**：对完整 UTF-8 响应正文搜索匹配，完全不依赖 JSON/XML 解析。
  `invert=true` 要求不匹配；默认 false。用 `^...$` 约束整串，
  `(?m)` 开启多行锚点，`(?s)` 允许点匹配换行。
- **XML**：使用 [SXD-XPath](https://docs.rs/sxd-xpath/latest/sxd_xpath/) 的 XPath 1.0 求值，
  支持元素、属性、文本、路径筛选及函数结果。节点按文档顺序读取其 XPath 字符串值；
  `XmlMatcher::Equals` 比较字符串，不裁剪空白，预期数字也应写字符串。
  同样支持 Regex、Exists、NotExists 与 One/Any/All。存在检查仅接受节点集合；
  `count(...)`、`boolean(...)` 等标量按 XPath 字符串转换后与预期值比较。
  `namespaces` 显式绑定查询前缀，包括查询 XML 默认命名空间时；
  `xml` 前缀自动绑定标准 URI。未绑定前缀在构造阶段报错。

所有组件按声明顺序执行；一项失败不会跳过其他项。JSON 与 XML 按需各解析一次，
互不阻塞；例如对 XML 响应配置 JSON 组件会失败，但正文正则和 XML 组件仍正常执行。
JSONPath、正则、XPath 语法及命名空间绑定在构造时检查。
XPath 的线程相关执行对象在当前线程内创建，不跨线程共享。

三种组件都受独立的 `max_assertion_body_bytes` 限制（默认 8 MiB），
不使用报告中可能截断的响应体。超限、缺失或读取不完整的输入不能通过负向断言。
正文正则与 XML 当前要求 UTF-8；XML 不支持 DOCTYPE/DTD、外部实体或 XSD 校验。
三种组件的表达式与预期值都是静态配置，不展开变量。

脚本中的组件使用 `type: json / regex / xml`。未写 type 的旧 JSON 断言继续兼容。
完整脚本见根目录 `examples/assertions.json`，配套本地服务为 `controller_http_server`。

## 条件控制器

`HttpNode::Controller`、`HttpNode::If`、`HttpNode::While` 编译为核心控制器。
子节点中的 Defaults 仍按作用域合并，条件保留到运行时求值。

```rust,no_run
use rmeter_component::{HttpNode, HttpRequestConfig, HttpRequestDefaults, compile_http_nodes};
use rmeter_core::controller::{Condition, WhileCondition};

let nodes = compile_http_nodes(vec![
    HttpNode::If {
        name: "是否轮询".into(),
        condition: "${enabled}".into(),
        evaluate_all: false,
        children: vec![HttpNode::While {
            name: "轮询".into(),
            condition: WhileCondition::Condition(Condition::LastSampleOk),
            children: vec![HttpNode::Request(HttpRequestConfig::with_url(
                "查询状态", "http://127.0.0.1:8080/status",
            ))],
        }],
    },
], &HttpRequestDefaults::default()).unwrap();
```

示例条件依赖用户变量 `enabled`，并在最后采样失败时退出 While。
若需要读取 HTTP 响应中的字段作为条件变量，需要另外配置响应提取逻辑。
控制器接口和完整条件语义见 `rmeter-core/README.md`。
