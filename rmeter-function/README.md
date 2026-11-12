# rmeter-function

Pure Rust implementations of `RandomString`, `changeCase`, `dateTimeConvert`,
`Random` and `RandomDate`, shared by Rust callers, expressions and the QuickJS
worker. No Java runtime is required to build, deploy or run rmeter.

## Calling functions

```javascript
functions.dateTimeConvert('2024-02-29 12:30:45', 'yyyy-MM-dd HH:mm:ss', 'dd/MM/yyyy', 'day');
functions.RandomString(16, 'abcdef0123456789', 'requestId');
functions.changeCase('hello', 'UPPER');
functions.Random('9223372036854775807', '9223372036854775807');
functions.RandomDate('', '2024-02-29', '2024-03-01', 'en_US', 'randomDay');
```

All five JavaScript functions return strings. Numeric arguments accept safe
integer Numbers or decimal strings. Other arguments must be strings. Optional
trailing arguments can be omitted or `undefined`. A nonblank final variable name
stores the result through `vars.put`, including its quota checks and script rollback.

Expressions use JMeter names and argument order:

```text
${__Random(1,100,number)}
${__dateTimeConvert(${createdAt},yyyy-MM-dd HH:mm:ss,yyyyMMdd,day)}
${__changeCase(a\,b\)c,UPPER,result)}-${result}
```

Expressions are supported wherever HTTP, JDBC, connection settings, timers and
IF/While already supported `${variable}`. Function arguments can contain nested
expressions and empty arguments; escape commas, parentheses and backslashes in
arguments with `\`. Values returned by variables/functions are never reparsed.
Functions run left to right, with writes visible to subsequent expressions in
the same string. A failed string expansion commits no writes from that string.
Different fields are evaluated separately. JSON string values expand, object
keys do not. JavaScript source is never interpolated; use `functions` directly.

## Semantics and bounds

| Function | Arguments | Behavior |
| --- | --- | --- |
| RandomString | length, chars?, variableName? | UTF-16 length as in JMeter; Unicode output; blank character set uses unrestricted characters, which can include control characters |
| changeCase | text, mode?, variableName? | UPPER (default), LOWER or CAPITALIZE; case-insensitive mode; capitalization affects only the first Unicode character |
| dateTimeConvert | text, sourceFormat, targetFormat, variableName? | Java pattern conversion; empty source means Unix epoch milliseconds; empty target produces an empty string |
| Random | min, max, variableName? | Inclusive signed 64-bit range; equal endpoints supported |
| RandomDate | format, start, end, locale?, variableName? | Uniform days in [start,end); blank format defaults to yyyy-MM-dd, blank start to today; end must be later |

Invalid input is an error, unlike JMeter's per-function logging/empty-string
fallbacks. JavaScript can catch errors; uncaught errors fail the owning sample.
Variable quota violations still fail the script even when caught. Expression
errors use the existing component error path. No function provides cryptographic
token guarantees.

Limits: 1 MiB per expression, argument and result; 32 nested expressions or date
optional sections; 262,144 UTF-16 units per random string; 4,096 bytes per date
pattern. Compiled expression and date caches have 256 entries; the locale cache
holds at most eight locales. Random source rejection is bounded. No evaluation
results or user variables are cached globally.

## Date compatibility

The baseline is JDK 21 `DateTimeFormatter.ofPattern` with ISO chronology and
SMART resolution, not Java's older `SimpleDateFormat`. All pattern letter
families are implemented, including text widths, eras, localized week dates,
quarters, day periods, nanoseconds, optional sections, padding, literals, zone
IDs/names and offsets. Calendar arithmetic covers years -999,999,999 through
+999,999,999. SMART can resolve February 30 to the month's last day and 24:00
to the next day; this is intentional Java behavior. A pattern requesting fields
absent from the parsed value fails unless those fields are in an optional section.

The system locale and IANA timezone are captured at run start and sent to the
worker. `Environment` lets Rust callers provide explicit values. `Source` allows
tests to inject a clock and random bits. Java `Builder` APIs, Java classes and
non-ISO chronologies are not exposed. `RandomDate` uses JMeter's default year,
month and day fields when the pattern omits them.

Bundled data metadata: `data/version.json`. Locale data includes all 1,068
locales from the pinned JDK, compressed separately for lazy loading. Zone data
includes historical wall/standard transitions, recurring rules and explicit DST
display-name overrides. The checked-in Java differential corpus covers all
pattern letter families, all bundled locales and all zone IDs, alongside parsing,
SMART, DST, negative timestamps and extreme-year cases.

## Development-only Java verification

Use the JDK 21 version recorded in `data/version.json`. Neither Cargo builds nor
normal tests invoke Java. Regenerating snapshots is an explicit maintainer action:

```powershell
$jdk = 'C:\Program Files\Microsoft\jdk-21.0.13.21-hotspot\bin'
& "$jdk/java.exe" --add-opens java.base/java.time.zone=ALL-UNNAMED --add-opens java.base/sun.util.locale.provider=ALL-UNNAMED -Xmx1500m rmeter-function/tools/ExportDateData.java rmeter-function/data
New-Item -ItemType Directory -Force target/date-tools | Out-Null
& "$jdk/javac.exe" -d target/date-tools rmeter-function/tools/ExportDateData.java rmeter-function/tools/DateFixtures.java
& "$jdk/java.exe" -Xmx1g -cp target/date-tools DateFixtures rmeter-function/tests/fixtures/java21.jsonl.gz
cargo test -p rmeter-function
```

The exporter reads JDK internals only to capture standard-offset transitions and
metazone DST overrides that are not available through the public zone-rule API.
It does not copy Java implementation code into the Rust runtime. Snapshot and
fixture version changes must be reviewed together. Data notices are in `data/licenses`.
