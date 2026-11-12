# rmeter-function

## Native functions

The five functions RandomString, changeCase, dateTimeConvert, Random and RandomDate are shared by Rust, JavaScript (`functions`) and `${__function(...)}` expressions. Date patterns follow JDK 21 ISO/SMART rules using bundled locale and timezone data; runtime Java is not needed. Defaults use the system timezone and locale captured at run start. Invalid calls fail explicitly. Optional variable names store results for the current user; failed expressions roll back their writes. JavaScript source is never interpolated. Controller evaluation now takes a mutable UserContext to support function writes.

```rust
use rmeter_function::{date_time_convert, Environment};
let env = Environment { timezone: "UTC".into(), locale: "en-US".into() };
assert_eq!(date_time_convert("2024-02-29", "yyyy-MM-dd", "dd/MM/yyyy", &env).unwrap(), "29/02/2024");
```
