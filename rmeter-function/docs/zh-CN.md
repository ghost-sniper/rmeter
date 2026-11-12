# rmeter-function

## 原生函数

RandomString、changeCase、dateTimeConvert、Random、RandomDate 由 Rust、JavaScript 的 `functions` 对象和 `${__函数(...)}` 表达式共用。日期模式采用 JDK 21 ISO/SMART 规则，地区和时区数据内置，运行时不需要 Java。默认时区和语言在运行开始时从系统读取。非法调用明确报错；可选变量名将结果写入当前用户，表达式失败时回滚其写入。JavaScript 源码不进行表达式替换。控制器求值接口现接收可变 UserContext，以支持函数写变量。

```rust
use rmeter_function::{date_time_convert, Environment};
let env = Environment { timezone: "UTC".into(), locale: "en-US".into() };
assert_eq!(date_time_convert("2024-02-29", "yyyy-MM-dd", "dd/MM/yyyy", &env).unwrap(), "29/02/2024");
```
