# rmeter

[English](crate) · [简体中文](crate::translations::zh_cn) · **繁體中文** · [日本語](crate::translations::ja) · [Français](crate::translations::fr) · [Deutsch](crate::translations::de)

crate 首頁和 API 參考預設使用英文。此使用指南提供六種語言；Rust 識別字及 API 參考註解保留英文。

## 概述

`rmeter` 是此 Rust/Tokio 負載測試工作區的指令碼入口，負責解析版本 1 的 JSON 計畫，編譯 HTTP、PostgreSQL 和 JavaScript 元件，並輸出執行事件。預設建置程式庫；啟用選用的 `cli` feature 後可使用命令列程式。

## 快速開始

在既有 Tokio runtime 內執行以下範例，並在範例位址啟動 HTTP 服務。必須同時消費事件：有界通道填滿後，引擎會等待。以 Cargo 本機路徑相依套件加入 `rmeter`，並為本例加入 `tokio` 和 `serde_json`。

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

## 執行與錯誤處理

[`parse_script`](crate::parse_script) 驗證並編譯計畫，不傳送 HTTP 請求，也不連線至 PostgreSQL；JavaScript 和原生 HTTP 擷取器即使在驗證時也需要獨立 worker。支援帶 BOM 的 UTF-8 JSON，拒絕未知欄位和不支援的版本。使用者變數為字串，依虛擬使用者隔離。

[`run_script`](crate::run_script) 和 [`run_script_file`](crate::run_script_file) 使用呼叫端的 runtime；丟棄執行中的 future 會取消虛擬使用者。取樣失敗透過事件回報，引擎仍可能回傳 `Ok(())`，因此須同時檢查 `success`、[`RunSummary`](crate::report::RunSummary) 和回傳結果。僅收到 `Finished` 不代表成功。事件索引從零開始，序列化時間戳記為 Unix 毫秒。

## API 導覽

- [`script`](crate::script)：解析、檔案載入和 [`ScriptError`](crate::ScriptError)。
- [`report`](crate::report)：可序列化事件與取樣計數。
- [`core`](crate::core)：計畫、控制器、上下文和引擎。
- [`component`](crate::component)：HTTP/JDBC/JavaScript 取樣器、斷言、擷取器和計時器。

## JavaScript 與擷取器 worker

從本工作區建置 worker，將同版本程式部署到宿主程式旁邊，或在編譯計畫前透過 `rmeter_component::javascript::set_worker_executable` 設定可信任的絕對路徑（使用統一入口時經由 `rmeter::component`）。也可由宿主設定 `RMETER_JS_WORKER`。JavaScript 節點、處理器和原生 HTTP 擷取器需要 worker；不使用這些功能的純 HTTP/JDBC 計畫不需要。worker 提供資源與故障隔離，並非完整的作業系統沙箱。

```sh
cargo build -p rmeter-component --release --bin rmeter-js-worker
```

## 命令列與文件

```sh
cargo run -p rmeter --features cli -- examples/http.json --check
cargo run -p rmeter --features cli -- examples/http.json
cargo doc --workspace --no-deps --open
cargo test --workspace --doc
```

## 原生函式

RandomString、changeCase、dateTimeConvert、Random、RandomDate 由 Rust、JavaScript 的 `functions` 物件和 `${__函式(...)}` 表達式共用。日期模式採用 JDK 21 ISO/SMART 規則，內建地區和時區資料，執行時不需要 Java。預設時區和語言於執行開始時從系統讀取。無效呼叫明確報錯；選填變數名稱會將結果寫入目前使用者，表達式失敗會回復其寫入。JavaScript 原始碼不進行表達式替換。控制器求值介面現接收可變 UserContext，以支援函式寫入變數。

## 控制器每輪逾時

Loop 與 While 每輪重新計時。JSON `timeout_ms` 為非負安全整數毫秒，省略或 0 表示無限制；null、小數和運算式無效。保留 JSON version 1。預算包含巢狀子節點、計時器、處理器與取樣器，排除結果通道阻塞。逾時取消目前取樣、退出整個控制器，並以控制器名稱產生一筆 `ControllerTimeout` 失敗紀錄，未命名 Loop 顯示「Loop 循环」。已完成取樣與變數保留，中斷取樣不重複計數。巢狀控制器最早到期者優先，同時到期則外層優先。`last_sample_ok` 設為 false，遵循執行緒群組錯誤策略；Continue 從逾時控制器之後繼續。使用者取消與群組執行期限維持原有語意。

Rust：新增 `Node::LoopController(LoopController::new(name, count, children))`；它與 `WhileController::timeout` 預設 `Duration::ZERO`，原 `Node::Loop` 保留不限時入口。`ComponentNode::LoopController` 包含 name/count/timeout/children，`ComponentNode::While` 也需提供 timeout。直接使用游標時處理 `CursorStep::Timeout`，以 `timeout_remaining()` 限制取樣 future，並用 `exclude_reporting_time()` 排除回報等待。
