# JDBC Connection Configuration 与 JDBC 采样器

沿用 JMeter 的组件命名，通过 Rust 原生 `tokio-postgres`、`deadpool-postgres` 和系统 TLS
连接 PostgreSQL，不运行 JVM、不加载 Java JDBC JAR。当前只支持 PostgreSQL。

## 脚本配置

将连接配置放在线程组或控制器的 children 中：

```json
{
  "type": "jdbc_connection_configuration",
  "name": "PostgreSQL 连接",
  "variable_name": "pg_pool",
  "driver": "postgresql",
  "url": "postgresql://postgres@127.0.0.1:5432/postgres?sslmode=disable",
  "password_env": "PGPASSWORD",
  "max_connections": 10,
  "connect_timeout_ms": 10000,
  "acquire_timeout_ms": 10000
}
```

配置作用于同层及后代，不受放置顺序影响，配置本身不产生采样事件。
子作用域可用同名配置覆盖外层，同层重复 variable_name 报错。
也可在计划顶层提供 `jdbc_connections` 数组，条目省略 type，在多个线程组之间共享池。

name、variable_name、url 必填，driver 可省略。URL 支持 `postgresql://`、`postgres://`
及兼容前缀 `jdbc:postgresql://`。可通过 username、password 单独提供登录信息；
password_env 从进程环境变量读取密码，与 password 互斥。URL/用户名/密码支持变量，
在首次采样时展开并惰性创建池，CLI `--var` 覆盖也生效。
初始化后同一个池的连接身份必须保持不变，不同身份请使用不同配置名。
配置 Debug 隐藏 URL 和密码。

默认池上限 10，可设置 1–65535。默认连接与获取超时均为 10 秒，必须大于 0。
URL 的 sslmode 控制 TLS（disable/prefer/require）；TLS 使用系统证书和主机名验证。
上面的 disable 用于本地示例。`--check` 只检查配置/引用/模板，不建立连接。

采样器只引用池名称，不接受连接 URL 或密码：

```json
{
  "type": "jdbc_request",
  "name": "查询用户",
  "connection": "pg_pool",
  "query_type": "select",
  "sql": "SELECT id, display_name FROM users WHERE id=$1",
  "parameters": [42],
  "query_timeout_ms": 30000,
  "max_rows": 1000,
  "max_result_bytes": 8388608,
  "save_response_body": true,
  "result_variable": "last_query_result",
  "row_count_variable": "selected_count",
  "assertions": [{"type":"json","name":"一行数据","path":"$.row_count","matcher":{"type":"equals","expected":1}}]
}
```

name、connection、sql 必填。query_type 为 select（默认，返回行）或 update（返回更新数）。
parameters 默认空，按 `$1`、`$2` 顺序绑定。字符串支持 `${variable}` 展开，数字、布尔、
null 及 JSON 对象/数组均通过协议独立传值，由 PostgreSQL 转换到推断的参数类型。
对象/数组适合 JSON/JSONB；PostgreSQL 原生数组参数可传其文本格式字符串。
SQL 字符串变量展开不负责 SQL 转义，数据值应放进 parameters。

每次采样执行单条预处理语句，在独立事务中完整读取结果后提交；
失败或结果超限时事务回滚。当前不支持跨采样事务、多语句批处理、存储过程 OUT 参数。
回收连接时清理会话状态（包括临时表），不支持跨采样持久临时表或会话变量。

## 结果与错误

响应为 JSON，例如：

```json
{"columns":["id","display_name"],"rows":[[42,"Alice"]],"row_count":1,"rows_affected":1}
```

行以数组保存，避免重复列名覆盖数据。Update 的 rows 为空、row_count 为 0。
结果支持整数、浮点、布尔、文本、NULL、JSON/JSONB、UUID、日期/时间、bytea、枚举、域与一维数组。
NUMERIC 转为字符串保留精度与小数位数；bytea 为 `\x` 十六进制字符串，日期/时间为字符串，
非有限浮点转为字符串。不支持的类型可在 SQL 中转换成 text。

max_rows 默认 1000、max_result_bytes 默认 8 MiB，超限失败而非静默截断；
query_timeout_ms 默认 30 秒，覆盖取连接、执行、读取和提交，必须大于 0，
且不超过 PostgreSQL statement_timeout 的整数毫秒范围。还设置事务内的服务端 statement_timeout。
超时或取消会从池移除当前连接，发送服务端取消请求，再关闭连接，避免污染后续采样。

result_variable 保存完整 JSON 字符串，row_count_variable 保存所选行数的字符串。
采样开始会清除这些变量的旧值；数据库执行失败时不会沿用旧结果。
save_response_body 默认 true，设为 false 不影响断言与结果变量。
现有 JSON/正文正则/XML 组件均可配置，JSON 结果通常使用 JSONPath 或正文正则。
断言失败保留查询结果，同时把采样 success 置为 false，参与错误策略和 IF/While。

成功响应码为 `00000`；数据库错误保留 SQLSTATE，例如未找到表为 `42P01`。
本地超时为 `TIMEOUT`，服务端语句超时可为 `57014`；配置错误为 `INVALID_CONFIG`，
结果超限为 `RESULT_LIMIT`。SQL/连接错误返回失败 SampleResult，保留采样名称。
bytes_sent 是展开的 SQL 与参数数据大小，bytes_received 是结果 JSON 大小，均不包含数据库协议开销。
elapsed 包含连接获取、执行、读取与提交，不包含断言时间。

## Rust 与验证

```rust,no_run
use std::sync::Arc;
use rmeter_component::{JdbcConnection, JdbcConnectionConfig, JdbcSampler, JdbcSamplerConfig};
use rmeter_core::{context::UserContext, sampler::Sampler};
# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let mut connection = JdbcConnectionConfig::new("db", "postgresql://user@localhost/test");
connection.password_env = Some("PGPASSWORD".into());
let connection = Arc::new(JdbcConnection::new(connection)?);
let mut request = JdbcSamplerConfig::new("查询", "db", "SELECT $1::integer");
request.parameters.push(serde_json::json!(42));
let sampler = JdbcSampler::new(request, connection)?;
let result = sampler.sample(&mut UserContext::new("数据库用户", 0)).await.unwrap();
assert!(result.success);
# Ok(())
# }
```

混合树使用 ComponentNode 与 ComponentCompiler，执行类型为 ComponentSampler；
使用 ComponentCompiler::register_jdbc 注册跨组共享的全局连接。
原有 HttpNode / compile_http_nodes 保持纯 HTTP Rust API。
完整脚本为根目录 `examples/jdbc_postgresql.json`，预期 4 次采样、5 项断言通过。

真实数据库集成测试默认忽略。在隔离测试库设置 `RMETER_POSTGRES_URL` 后执行：

```sh
cargo test -p rmeter-component --test jdbc -- --include-ignored
```
