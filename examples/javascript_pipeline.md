# JavaScript 变量联动示例

[javascript_pipeline.json](javascript_pipeline.json) 运行 2 个虚拟用户，每个用户 2 次迭代。
JavaScript 用 ES6 解构、箭头函数和模板字符串生成 request ID，并通过 `vars.put` 保存计数和分支变量。
IF 读取脚本设置的开关，HTTP 将 request ID 作为查询参数发送给本地服务；
While 读取脚本设置的循环次数，JDBC 将计数和 request ID 绑定为 PostgreSQL 参数。
JDBC 只执行 SELECT，无需建表。

先构建 CLI、JavaScript worker 和本地 HTTP 服务：

```sh
cargo build -p rmeter-component --bin rmeter-js-worker
cargo build -p rmeter --features cli --bin rmeter --example controller_http_server
```

在单独的终端启动 HTTP 服务，收到 4 次请求后退出：

```sh
cargo run -p rmeter --example controller_http_server -- 127.0.0.1:8080 4
```

准备本地 PostgreSQL，按实际数据库地址覆盖 `db_url`：

```sh
cargo run -p rmeter --features cli --bin rmeter -- examples/javascript_pipeline.json --check
cargo run -p rmeter --features cli --bin rmeter -- examples/javascript_pipeline.json --var "db_url=postgresql://postgres@127.0.0.1:5432/postgres?sslmode=disable"
```

预期 12 次采样、12 项断言全部通过；`/must-not-run` 分支不执行。
HTTP 服务应收到 `user-0-1`、`user-0-2`、`user-1-1`、`user-1-2`（顺序可变）。
同一个用户的计数从 1 增至 2，两个用户互不影响。
JDBC 返回行的两列是数字计数和同一个 request ID。
`sslmode=disable` 仅用于本地示例，远程 PostgreSQL 请使用 TLS。
