# rmeter-component

[English](crate) · [简体中文](crate::translations::zh_cn) · **繁體中文** · [日本語](crate::translations::ja) · [Français](crate::translations::fr) · [Deutsch](crate::translations::de)

crate 首頁和 API 參考預設使用英文。此使用指南提供六種語言；Rust 識別字及 API 參考註解保留英文。

## 概述

`rmeter-component` 為 `rmeter-core` 提供 HTTP/HTTPS、PostgreSQL（採用 JDBC 風格命名）和同步 JavaScript 取樣器，另提供 JSONPath、正規表示式及 XML 斷言、HTTP 回應擷取器、作用域預設值和常數/高斯計時器。PostgreSQL 使用 Rust 原生驅動程式，不載入 JVM 或 Java JDBC 驅動程式。

## 快速開始

在 Tokio runtime 內執行範例，本機服務應回傳 `{"ready":true}`。建構函式驗證靜態設定；只有等待 `sample` 時才傳送 HTTP 請求。

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

## 執行與錯誤處理

[`ComponentCompiler`](crate::ComponentCompiler) 將詞法作用域預設值、具名資料庫連線池和計時器編譯為執行節點。內層預設值覆蓋外層，請求明確設定覆蓋繼承值。請求明確指定的 header/query 清單取代繼承清單。相容的 HTTP 用戶端與具名資料庫連線池可共用，使用者變數保持隔離。HTTP 不自動儲存 Cookie。

處理順序為前置處理器 → 計時器 → 取樣器 → 後置處理器 → 斷言。處理器不產生獨立取樣事件；處理器或斷言失敗會使父取樣失敗。JavaScript 僅在成功時提交變數修改；變數回復不撤銷外部資料庫操作。後置處理器透過唯讀 `prev` 讀取斷言前的目前結果。

HTTP 報告儲存、斷言輸入、後置輸入分別設限。截斷儲存的回應內容不會作為完整斷言輸入。HTTP 執行錯誤以失敗的 `SampleResult` 回傳，即使 `sample` 回傳 `Ok` 也須檢查 `success`。

## API 導覽

- [`http`](crate::http)：請求、內容、預設值和純 HTTP 樹編譯。
- [`jdbc`](crate::jdbc)：具名 PostgreSQL 連線池及參數化查詢。
- [`javascript`](crate::javascript)：指令碼、限制和 worker 設定。
- [`assertion`](crate::assertion) / [`extractor`](crate::extractor)：回應檢查與變數擷取。
- [`component`](crate::component) / [`timer`](crate::timer)：混合樹和延遲。

## JavaScript 與擷取器 worker

從本工作區建置 worker，將同版本程式部署到宿主程式旁邊，或在編譯計畫前透過 `rmeter_component::javascript::set_worker_executable` 設定可信任的絕對路徑（使用統一入口時經由 `rmeter::component`）。也可由宿主設定 `RMETER_JS_WORKER`。JavaScript 節點、處理器和原生 HTTP 擷取器需要 worker；不使用這些功能的純 HTTP/JDBC 計畫不需要。worker 提供資源與故障隔離，並非完整的作業系統沙箱。

```sh
cargo build -p rmeter-component --release --bin rmeter-js-worker
```

## 原生函式

RandomString、changeCase、dateTimeConvert、Random、RandomDate 由 Rust、JavaScript 的 `functions` 物件和 `${__函式(...)}` 表達式共用。日期模式採用 JDK 21 ISO/SMART 規則，內建地區和時區資料，執行時不需要 Java。預設時區和語言於執行開始時從系統讀取。無效呼叫明確報錯；選填變數名稱會將結果寫入目前使用者，表達式失敗會回復其寫入。JavaScript 原始碼不進行表達式替換。控制器求值介面現接收可變 UserContext，以支援函式寫入變數。

## 控制器每輪逾時

Loop 與 While 每輪重新計時。JSON `timeout_ms` 為非負安全整數毫秒，省略或 0 表示無限制；null、小數和運算式無效。保留 JSON version 1。預算包含巢狀子節點、計時器、處理器與取樣器，排除結果通道阻塞。逾時取消目前取樣、退出整個控制器，並以控制器名稱產生一筆 `ControllerTimeout` 失敗紀錄，未命名 Loop 顯示「Loop 循环」。已完成取樣與變數保留，中斷取樣不重複計數。巢狀控制器最早到期者優先，同時到期則外層優先。`last_sample_ok` 設為 false，遵循執行緒群組錯誤策略；Continue 從逾時控制器之後繼續。使用者取消與群組執行期限維持原有語意。

Rust：新增 `Node::LoopController(LoopController::new(name, count, children))`；它與 `WhileController::timeout` 預設 `Duration::ZERO`，原 `Node::Loop` 保留不限時入口。`ComponentNode::LoopController` 包含 name/count/timeout/children，`ComponentNode::While` 也需提供 timeout。直接使用游標時處理 `CursorStep::Timeout`，以 `timeout_remaining()` 限制取樣 future，並用 `exclude_reporting_time()` 排除回報等待。
