# 条件与循环控制器 HTTP 测试

测试脚本为 `examples/controllers_http.json`，使用本地 HTTP 服务，无需访问外部站点。

每个虚拟用户的每一轮执行：

1. 通用 Controller 顺序执行子节点。
2. IF 检查 `${enabled}`；默认 true，进入 Loop，发送 3 次 `/status` 请求。
3. 第二个 IF 条件为 false，跳过 `/must-not-run` 请求。
4. While `poll` 检查循环索引小于 2，分别发送 `/poll?index=0` 和 `/poll?index=1`。
5. 每个实际请求执行 2 项 JSONPath 断言。

线程组设置为 2 个用户、每用户 2 轮，总计 `2 × 2 × (3 + 2) = 20` 次 HTTP 采样，
其中状态请求 12 次、轮询请求 8 次、跳过分支 0 次；断言共 40 项。

在第一个终端启动示例服务：

```sh
cargo run -p rmeter --example controller_http_server
```

默认监听 `127.0.0.1:8080`，按 Ctrl+C 停止。支持指定监听地址和可选的请求总数；
例如使用空闲端口，并在处理 20 次请求后自动退出：

```sh
cargo run -p rmeter --example controller_http_server -- 127.0.0.1:0 20
```

服务启动时输出实际 `base_url`。在第二个终端检查并执行脚本：

```sh
cargo run -p rmeter --features cli -- examples/controllers_http.json --check
cargo run -p rmeter --features cli -- examples/controllers_http.json
```

若使用其他端口，执行时覆盖变量，例如：

```sh
cargo run -p rmeter --features cli -- examples/controllers_http.json --var base_url=http://127.0.0.1:9000
```

通过时 CLI 退出码为 0，最终 summary 的 `samples=20`、`passed=20`、`failed=0`、
`finished=true`、`cancelled=false`、`error=null`。
修改 `--var enabled=false` 可跳过整个 Loop，此时只执行 8 次 While 请求；
若示例服务使用了自动退出计数，也应将计数改为 8。

示例服务仅用于测试这些 GET 路由，输出实际接收路径及循环索引，方便与采样事件核对。
