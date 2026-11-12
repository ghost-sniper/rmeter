# rmeter-component

[English](crate) · [简体中文](crate::translations::zh_cn) · [繁體中文](crate::translations::zh_tw) · [日本語](crate::translations::ja) · [Français](crate::translations::fr) · **Deutsch**

Die Crate-Startseite und die API-Referenz sind standardmäßig englisch. Diese Anleitungen sind in sechs Sprachen verfügbar; Rust-Bezeichner und API-Kommentare bleiben englisch.

## Überblick

`rmeter-component` liefert HTTP/HTTPS-, PostgreSQL- (mit JDBC-artigen Komponentennamen) und synchrone JavaScript-Sampler für `rmeter-core`. Dazu kommen JSONPath-, Regex- und XML-Assertions, HTTP-Extraktoren, bereichsbezogene Standardwerte und konstante/Gauß-Timer. PostgreSQL verwendet native Rust-Treiber; JVM und Java-JDBC-Treiber werden nicht geladen.

## Schnellstart

Führen Sie das Beispiel in Tokio gegen einen lokalen Dienst aus, der `{"ready":true}` zurückgibt. Konstruktoren prüfen die statische Konfiguration; die HTTP-Anfrage wird erst beim Warten auf `sample` gesendet.

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

## Ausführung und Fehler

[`ComponentCompiler`](crate::ComponentCompiler) löst Standardwerte, benannte Datenbankpools und Timer zu ausführbaren Knoten auf. Innere Standardwerte überschreiben äußere, explizite Anfrageeinstellungen überschreiben geerbte Werte. Explizite header/query-Listen ersetzen geerbte Listen vollständig. Kompatible HTTP-Clients und benannte Pools werden geteilt, Benutzervariablen bleiben getrennt. HTTP-Cookies werden nicht automatisch gespeichert.

Die Reihenfolge lautet Vorverarbeitung → Timer → Sampler → Nachverarbeitung → Assertions. Prozessoren erzeugen keine eigenen Sample-Ereignisse. Fehler eines Prozessors oder einer Assertion lassen das übergeordnete Sample fehlschlagen. JavaScript übernimmt Variablenänderungen nur bei Erfolg; ein Zurücksetzen macht externe Datenbankoperationen nicht rückgängig. Die Nachverarbeitung liest das Ergebnis vor Assertions über das schreibgeschützte `prev`.

Gespeicherter HTTP-Body, Assertion-Eingabe und Nachverarbeitungseingabe haben eigene Limits. Ein gekürzter gespeicherter Body gilt nicht als vollständige Assertion-Eingabe. HTTP-Laufzeitfehler werden als fehlgeschlagenes `SampleResult` geliefert: Prüfen Sie `success`, auch wenn `sample` `Ok` zurückgibt.

## API-Übersicht

- [`http`](crate::http): Anfragen, Bodies, Standardwerte und HTTP-Bäume.
- [`jdbc`](crate::jdbc): benannte PostgreSQL-Pools und parametrisierte Abfragen.
- [`javascript`](crate::javascript): Skripte, Limits und Worker.
- [`assertion`](crate::assertion) / [`extractor`](crate::extractor): Antwortprüfung und Variablenextraktion.
- [`component`](crate::component) / [`timer`](crate::timer): gemischte Bäume und Wartezeiten.

## Worker für JavaScript und Extraktoren

Bauen Sie den Worker in diesem Workspace und legen Sie die ausführbare Datei derselben Version neben die Hostanwendung. Alternativ setzen Sie vor dem Kompilieren von Plänen einen vertrauenswürdigen absoluten Pfad mit `rmeter_component::javascript::set_worker_executable` (über `rmeter::component` bei Nutzung der Fassade). Auch `RMETER_JS_WORKER` kann vom Host gesetzt werden. JavaScript, Prozessoren und native HTTP-Extraktoren benötigen den Worker; reine HTTP/JDBC-Pläne ohne diese Funktionen nicht. Der Worker begrenzt Ressourcen und isoliert Fehler, ist jedoch keine vollständige Betriebssystem-Sandbox.

```sh
cargo build -p rmeter-component --release --bin rmeter-js-worker
```

## Native Funktionen

RandomString, changeCase, dateTimeConvert, Random und RandomDate werden von Rust, JavaScript (`functions`) und `${__function(...)}`-Ausdrücken gemeinsam verwendet. Datumsformate folgen den ISO/SMART-Regeln von JDK 21; Sprach- und Zeitzonendaten sind eingebettet. Java wird zur Laufzeit nicht benötigt. Systemvorgaben werden beim Start eines Laufs festgehalten. Ungültige Aufrufe liefern Fehler. Ein optionaler Variablenname speichert das Ergebnis für den aktuellen Benutzer; bei Fehlern werden Schreibzugriffe des Ausdrucks zurückgenommen. JavaScript-Quelltext wird nicht interpoliert. Die Controller-Auswertung erhält jetzt einen veränderbaren UserContext für Variablenzugriffe.

## Zeitlimit je Controller-Durchlauf

Loop und While setzen ihr Zeitbudget bei jedem Durchlauf zurück. JSON `timeout_ms` ist eine nichtnegative sichere Ganzzahl in Millisekunden; fehlend oder 0 bedeutet unbegrenzt. null, Brüche und Ausdrücke sind ungültig. JSON bleibt Version 1. Das Budget umfasst verschachtelte Knoten, Timer, Prozessoren und Sampler, jedoch keine Wartezeit beim Übermitteln von Ergebnissen. Bei Ablauf wird der aktive Sampler abgebrochen, der gesamte Controller verlassen und genau ein Fehler `ControllerTimeout` unter dem Controllernamen gemeldet (unbenannter Loop: „Loop 循环“). Fertige Ergebnisse und Variablen bleiben erhalten; abgebrochene Samples werden nicht doppelt gezählt. Die früheste Frist gewinnt, bei Gleichstand der äußere Controller. `last_sample_ok` wird false und die Fehlerstrategie der Thread-Gruppe greift; Continue fährt nach dem Controller fort. Benutzerabbruch und Gruppenlaufzeit behalten ihre bisherige Bedeutung.

Rust: `Node::LoopController(LoopController::new(name, count, children))` und `WhileController::timeout` verwenden standardmäßig `Duration::ZERO`; `Node::Loop` bleibt unbegrenzt. `ComponentNode::LoopController` enthält name/count/timeout/children; `ComponentNode::While` benötigt ebenfalls timeout. Direkte Cursor-Nutzer behandeln `CursorStep::Timeout`, begrenzen Sampler-Futures mit `timeout_remaining()` und ziehen Berichtswartezeiten mit `exclude_reporting_time()` ab.
