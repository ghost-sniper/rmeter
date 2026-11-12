# rmeter-function

## 原生函式

RandomString、changeCase、dateTimeConvert、Random、RandomDate 由 Rust、JavaScript 的 `functions` 物件和 `${__函式(...)}` 表達式共用。日期模式採用 JDK 21 ISO/SMART 規則，內建地區和時區資料，執行時不需要 Java。預設時區和語言於執行開始時從系統讀取。無效呼叫明確報錯；選填變數名稱會將結果寫入目前使用者，表達式失敗會回復其寫入。JavaScript 原始碼不進行表達式替換。控制器求值介面現接收可變 UserContext，以支援函式寫入變數。

```rust
use rmeter_function::{date_time_convert, Environment};
let env = Environment { timezone: "UTC".into(), locale: "en-US".into() };
assert_eq!(date_time_convert("2024-02-29", "yyyy-MM-dd", "dd/MM/yyyy", &env).unwrap(), "29/02/2024");
```
