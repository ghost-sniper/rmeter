# rmeter-core

[English](crate) · [简体中文](crate::translations::zh_cn) · **繁體中文** · [日本語](crate::translations::ja) · [Français](crate::translations::fr) · [Deutsch](crate::translations::de)

crate 首頁和 API 參考預設使用英文。此使用指南提供六種語言；Rust 識別字及 API 參考註解保留英文。

## 概述

`rmeter-core` 定義與協定無關的執行計畫、虛擬使用者狀態、取樣與斷言 trait、控制器和 Tokio 引擎。實作自訂取樣器或建立強型別計畫時使用本 crate；載入 JSON 指令碼使用 `rmeter`，內建傳輸元件使用 `rmeter-component`。

## 快速開始

以下獨立範例遍歷條件計畫，無須網路。遍歷時 `Node::Sample` 可持有任意值；透過 `Engine` 執行時，取樣型別須實作 `Sampler`。

```rust
use rmeter_core::{
    context::UserContext,
    controller::{CursorStep, ExecutionCursor, IfController},
    plan::Node,
};

let nodes = vec![Node::If(IfController::new(
    "Enabled", "${enabled}", vec![Node::Sample("health")],
))];
let mut user = UserContext::new("Users", 0);
user.variables.insert("enabled".into(), "true".into());
let mut cursor = ExecutionCursor::new(&nodes);
assert!(matches!(cursor.step(&mut user).unwrap(), CursorStep::Sample(&"health")));
assert!(matches!(cursor.step(&mut user).unwrap(), CursorStep::Done));
```

## 執行與錯誤處理

建立包含 [`ThreadGroup`](crate::plan::ThreadGroup) 的 [`TestPlan`](crate::plan::TestPlan)，在 Tokio 內呼叫 [`Engine::run`](crate::engine::Engine::run)，同時消費有界事件通道。每個使用者擁有獨立變數和控制器游標。使用者索引、執行緒群組迭代索引從零開始；變數在該使用者後續取樣和迭代間保留。

[`ErrorPolicy`](crate::plan::ErrorPolicy) 決定取樣失敗後繼續、進入下次迭代、停止使用者或停止測試。基礎設施與條件求值錯誤回傳 [`EngineError`](crate::engine::EngineError)。斷言通過不能恢復既有失敗。丟棄執行中的 future 會中止使用者工作。

IF 運算式僅將不區分大小寫的 `true` 視為真。While 字串條件在不區分大小寫等於 `false` 時停止；空字串和 `LAST` 使用特殊的上次取樣語意。直接使用 [`ExecutionCursor`](crate::controller::ExecutionCursor) 時，收到 `CursorStep::Yield` 應讓出非同步排程，確保無取樣的無限迴圈可取消。

## API 導覽

- [`plan`](crate::plan)：不可變計畫和錯誤策略。
- [`sampler`](crate::sampler) / [`assertion`](crate::assertion)：擴充 trait 與結果。
- [`context`](crate::context)：使用者變數和有界指令碼日誌。
- [`controller`](crate::controller)：順序、IF、While 和遍歷。
- [`engine`](crate::engine) / [`event`](crate::event)：並行執行與事件。

## 原生函式

RandomString、changeCase、dateTimeConvert、Random、RandomDate 由 Rust、JavaScript 的 `functions` 物件和 `${__函式(...)}` 表達式共用。日期模式採用 JDK 21 ISO/SMART 規則，內建地區和時區資料，執行時不需要 Java。預設時區和語言於執行開始時從系統讀取。無效呼叫明確報錯；選填變數名稱會將結果寫入目前使用者，表達式失敗會回復其寫入。JavaScript 原始碼不進行表達式替換。控制器求值介面現接收可變 UserContext，以支援函式寫入變數。

## 控制器每輪逾時

Loop 與 While 每輪重新計時。JSON `timeout_ms` 為非負安全整數毫秒，省略或 0 表示無限制；null、小數和運算式無效。保留 JSON version 1。預算包含巢狀子節點、計時器、處理器與取樣器，排除結果通道阻塞。逾時取消目前取樣、退出整個控制器，並以控制器名稱產生一筆 `ControllerTimeout` 失敗紀錄，未命名 Loop 顯示「Loop 循环」。已完成取樣與變數保留，中斷取樣不重複計數。巢狀控制器最早到期者優先，同時到期則外層優先。`last_sample_ok` 設為 false，遵循執行緒群組錯誤策略；Continue 從逾時控制器之後繼續。使用者取消與群組執行期限維持原有語意。

Rust：新增 `Node::LoopController(LoopController::new(name, count, children))`；它與 `WhileController::timeout` 預設 `Duration::ZERO`，原 `Node::Loop` 保留不限時入口。`ComponentNode::LoopController` 包含 name/count/timeout/children，`ComponentNode::While` 也需提供 timeout。直接使用游標時處理 `CursorStep::Timeout`，以 `timeout_remaining()` 限制取樣 future，並用 `exclude_reporting_time()` 排除回報等待。
