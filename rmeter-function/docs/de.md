# rmeter-function

## Native Funktionen

RandomString, changeCase, dateTimeConvert, Random und RandomDate werden von Rust, JavaScript (`functions`) und `${__function(...)}`-Ausdrücken gemeinsam verwendet. Datumsformate folgen den ISO/SMART-Regeln von JDK 21; Sprach- und Zeitzonendaten sind eingebettet. Java wird zur Laufzeit nicht benötigt. Systemvorgaben werden beim Start eines Laufs festgehalten. Ungültige Aufrufe liefern Fehler. Ein optionaler Variablenname speichert das Ergebnis für den aktuellen Benutzer; bei Fehlern werden Schreibzugriffe des Ausdrucks zurückgenommen. JavaScript-Quelltext wird nicht interpoliert. Die Controller-Auswertung erhält jetzt einen veränderbaren UserContext für Variablenzugriffe.

```rust
use rmeter_function::{date_time_convert, Environment};
let env = Environment { timezone: "UTC".into(), locale: "en-US".into() };
assert_eq!(date_time_convert("2024-02-29", "yyyy-MM-dd", "dd/MM/yyyy", &env).unwrap(), "29/02/2024");
```
