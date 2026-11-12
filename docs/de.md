# rmeter

[English](crate) · [简体中文](crate::translations::zh_cn) · [繁體中文](crate::translations::zh_tw) · [日本語](crate::translations::ja) · [Français](crate::translations::fr) · **Deutsch**

Die Crate-Startseite und die API-Referenz sind standardmäßig englisch. Diese Anleitungen sind in sechs Sprachen verfügbar; Rust-Bezeichner und API-Kommentare bleiben englisch.

## Überblick

`rmeter` ist der skriptbasierte Einstiegspunkt dieses Rust/Tokio-Workspaces für Lasttests. Es liest JSON-Testpläne der Version 1, kompiliert HTTP-, PostgreSQL- und JavaScript-Komponenten und liefert Laufzeitereignisse. Standardmäßig wird eine Bibliothek gebaut; das optionale Feature `cli` aktiviert das Kommandozeilenprogramm.

## Schnellstart

Führen Sie das Beispiel in einer vorhandenen Tokio-Runtime mit einem HTTP-Dienst unter der angegebenen URL aus. Ereignisse müssen gleichzeitig empfangen werden: Ein voller begrenzter Kanal hält die Engine an. Fügen Sie `rmeter` als lokale Cargo-Pfadabhängigkeit sowie `tokio` und `serde_json` für dieses Beispiel hinzu.

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

## Ausführung und Fehler

[`parse_script`](crate::parse_script) prüft und kompiliert ohne HTTP-Anfragen oder PostgreSQL-Verbindungen. JavaScript und native HTTP-Extraktoren benötigen jedoch bereits zur Prüfung den separaten Worker. UTF-8-JSON darf mit einer BOM beginnen. Unbekannte Felder und nicht unterstützte Versionen werden abgelehnt. Variablen sind Zeichenketten und pro virtuellem Benutzer getrennt.

[`run_script`](crate::run_script) und [`run_script_file`](crate::run_script_file) verwenden die Runtime des Aufrufers. Das Verwerfen der laufenden Future bricht virtuelle Benutzer ab. Fehlgeschlagene Samples werden als Ereignisse gemeldet; die Engine kann trotzdem `Ok(())` zurückgeben. Prüfen Sie `success`, [`RunSummary`](crate::report::RunSummary) und den Rückgabewert. `Finished` allein garantiert keinen Erfolg. Indizes beginnen bei null; serialisierte Zeitstempel sind Unix-Millisekunden.

## API-Übersicht

- [`script`](crate::script): Einlesen, Dateizugriff und [`ScriptError`](crate::ScriptError).
- [`report`](crate::report): serialisierbare Ereignisse und Zähler.
- [`core`](crate::core): Pläne, Controller, Kontexte und Engine.
- [`component`](crate::component): HTTP/JDBC/JavaScript-Sampler, Assertions, Extraktoren und Timer.

## Worker für JavaScript und Extraktoren

Bauen Sie den Worker in diesem Workspace und legen Sie die ausführbare Datei derselben Version neben die Hostanwendung. Alternativ setzen Sie vor dem Kompilieren von Plänen einen vertrauenswürdigen absoluten Pfad mit `rmeter_component::javascript::set_worker_executable` (über `rmeter::component` bei Nutzung der Fassade). Auch `RMETER_JS_WORKER` kann vom Host gesetzt werden. JavaScript, Prozessoren und native HTTP-Extraktoren benötigen den Worker; reine HTTP/JDBC-Pläne ohne diese Funktionen nicht. Der Worker begrenzt Ressourcen und isoliert Fehler, ist jedoch keine vollständige Betriebssystem-Sandbox.

```sh
cargo build -p rmeter-component --release --bin rmeter-js-worker
```

## CLI und Dokumentation

```sh
cargo run -p rmeter --features cli -- examples/http.json --check
cargo run -p rmeter --features cli -- examples/http.json
cargo doc --workspace --no-deps --open
cargo test --workspace --doc
```

## Native Funktionen

RandomString, changeCase, dateTimeConvert, Random und RandomDate werden von Rust, JavaScript (`functions`) und `${__function(...)}`-Ausdrücken gemeinsam verwendet. Datumsformate folgen den ISO/SMART-Regeln von JDK 21; Sprach- und Zeitzonendaten sind eingebettet. Java wird zur Laufzeit nicht benötigt. Systemvorgaben werden beim Start eines Laufs festgehalten. Ungültige Aufrufe liefern Fehler. Ein optionaler Variablenname speichert das Ergebnis für den aktuellen Benutzer; bei Fehlern werden Schreibzugriffe des Ausdrucks zurückgenommen. JavaScript-Quelltext wird nicht interpoliert. Die Controller-Auswertung erhält jetzt einen veränderbaren UserContext für Variablenzugriffe.

## Zeitlimit je Controller-Durchlauf

Loop und While setzen ihr Zeitbudget bei jedem Durchlauf zurück. JSON `timeout_ms` ist eine nichtnegative sichere Ganzzahl in Millisekunden; fehlend oder 0 bedeutet unbegrenzt. null, Brüche und Ausdrücke sind ungültig. JSON bleibt Version 1. Das Budget umfasst verschachtelte Knoten, Timer, Prozessoren und Sampler, jedoch keine Wartezeit beim Übermitteln von Ergebnissen. Bei Ablauf wird der aktive Sampler abgebrochen, der gesamte Controller verlassen und genau ein Fehler `ControllerTimeout` unter dem Controllernamen gemeldet (unbenannter Loop: „Loop 循环“). Fertige Ergebnisse und Variablen bleiben erhalten; abgebrochene Samples werden nicht doppelt gezählt. Die früheste Frist gewinnt, bei Gleichstand der äußere Controller. `last_sample_ok` wird false und die Fehlerstrategie der Thread-Gruppe greift; Continue fährt nach dem Controller fort. Benutzerabbruch und Gruppenlaufzeit behalten ihre bisherige Bedeutung.

Rust: `Node::LoopController(LoopController::new(name, count, children))` und `WhileController::timeout` verwenden standardmäßig `Duration::ZERO`; `Node::Loop` bleibt unbegrenzt. `ComponentNode::LoopController` enthält name/count/timeout/children; `ComponentNode::While` benötigt ebenfalls timeout. Direkte Cursor-Nutzer behandeln `CursorStep::Timeout`, begrenzen Sampler-Futures mit `timeout_remaining()` und ziehen Berichtswartezeiten mit `exclude_reporting_time()` ab.
