# rmeter-function

## ネイティブ関数

RandomString、changeCase、dateTimeConvert、Random、RandomDate は Rust、JavaScript の `functions` オブジェクト、`${__function(...)}` 式で共有します。日付パターンは JDK 21 の ISO/SMART 規則に従い、ロケールとタイムゾーンのデータを同梱するため実行時に Java は不要です。既定値は実行開始時のシステム設定です。不正な呼び出しはエラーとなります。省略可能な変数名で現在のユーザーに結果を保存し、式が失敗すると書き込みを取り消します。JavaScript ソースは式展開しません。コントローラーの評価は変数を書き込むため可変 UserContext を受け取ります。

```rust
use rmeter_function::{date_time_convert, Environment};
let env = Environment { timezone: "UTC".into(), locale: "en-US".into() };
assert_eq!(date_time_convert("2024-02-29", "yyyy-MM-dd", "dd/MM/yyyy", &env).unwrap(), "29/02/2024");
```
