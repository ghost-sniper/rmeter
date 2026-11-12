# rmeter-core

[English](crate) · [简体中文](crate::translations::zh_cn) · [繁體中文](crate::translations::zh_tw) · [日本語](crate::translations::ja) · [Français](crate::translations::fr) · **Deutsch**

Die Crate-Startseite und die API-Referenz sind standardmäßig englisch. Diese Anleitungen sind in sechs Sprachen verfügbar; Rust-Bezeichner und API-Kommentare bleiben englisch.

## Überblick

`rmeter-core` definiert protokollunabhängige Pläne, den Zustand virtueller Benutzer, Sampler- und Assertion-Traits, Controller und eine Tokio-Engine. Verwenden Sie diese Crate für eigene Sampler oder typisierte Pläne, `rmeter` zum Laden von JSON und `rmeter-component` für eingebaute Transportkomponenten.

## Schnellstart

Dieses eigenständige Beispiel durchläuft einen bedingten Plan ohne Netzwerkzugriff. Beim Durchlaufen darf `Node::Sample` beliebige Werte enthalten; für die Ausführung mit `Engine` muss der Sample-Typ `Sampler` implementieren.

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

## Ausführung und Fehler

Erstellen Sie einen [`TestPlan`](crate::plan::TestPlan) mit [`ThreadGroup`](crate::plan::ThreadGroup)-Einträgen und rufen Sie [`Engine::run`](crate::engine::Engine::run) in Tokio auf, während Sie den begrenzten Ereigniskanal gleichzeitig auslesen. Jeder Benutzer besitzt eigene Variablen und einen Controller-Cursor. Benutzer- und Gruppeniterationsindizes beginnen bei null; Variablen bleiben zwischen Samples und Iterationen desselben Benutzers erhalten.

[`ErrorPolicy`](crate::plan::ErrorPolicy) bestimmt nach einem Fehler: fortfahren, nächste Iteration, Benutzer stoppen oder Test stoppen. Infrastruktur- und Bedingungsfehler liefern [`EngineError`](crate::engine::EngineError). Erfolgreiche Assertions beheben keine vorherigen Sample-Fehler. Das Verwerfen der Future bricht Benutzer-Tasks ab.

IF-Ausdrücke akzeptieren nur `true`, unabhängig von Groß-/Kleinschreibung. While-Zeichenketten stoppen bei `false`; die leere Zeichenkette und `LAST` haben besondere Regeln zum letzten Sample. Bei direkter Nutzung von [`ExecutionCursor`](crate::controller::ExecutionCursor) müssen Sie bei `CursorStep::Yield` an die Runtime abgeben, damit Endlosschleifen ohne Samples abbrechbar bleiben.

## API-Übersicht

- [`plan`](crate::plan): unveränderliche Pläne und Fehlerstrategien.
- [`sampler`](crate::sampler) / [`assertion`](crate::assertion): Erweiterungs-Traits und Ergebnisse.
- [`context`](crate::context): Benutzervariablen und begrenzte Skriptprotokolle.
- [`controller`](crate::controller): Sequenzen, IF, While und Durchlauf.
- [`engine`](crate::engine) / [`event`](crate::event): nebenläufige Ausführung und Ereignisse.

## Native Funktionen

RandomString, changeCase, dateTimeConvert, Random und RandomDate werden von Rust, JavaScript (`functions`) und `${__function(...)}`-Ausdrücken gemeinsam verwendet. Datumsformate folgen den ISO/SMART-Regeln von JDK 21; Sprach- und Zeitzonendaten sind eingebettet. Java wird zur Laufzeit nicht benötigt. Systemvorgaben werden beim Start eines Laufs festgehalten. Ungültige Aufrufe liefern Fehler. Ein optionaler Variablenname speichert das Ergebnis für den aktuellen Benutzer; bei Fehlern werden Schreibzugriffe des Ausdrucks zurückgenommen. JavaScript-Quelltext wird nicht interpoliert. Die Controller-Auswertung erhält jetzt einen veränderbaren UserContext für Variablenzugriffe.

## Zeitlimit je Controller-Durchlauf

Loop und While setzen ihr Zeitbudget bei jedem Durchlauf zurück. JSON `timeout_ms` ist eine nichtnegative sichere Ganzzahl in Millisekunden; fehlend oder 0 bedeutet unbegrenzt. null, Brüche und Ausdrücke sind ungültig. JSON bleibt Version 1. Das Budget umfasst verschachtelte Knoten, Timer, Prozessoren und Sampler, jedoch keine Wartezeit beim Übermitteln von Ergebnissen. Bei Ablauf wird der aktive Sampler abgebrochen, der gesamte Controller verlassen und genau ein Fehler `ControllerTimeout` unter dem Controllernamen gemeldet (unbenannter Loop: „Loop 循环“). Fertige Ergebnisse und Variablen bleiben erhalten; abgebrochene Samples werden nicht doppelt gezählt. Die früheste Frist gewinnt, bei Gleichstand der äußere Controller. `last_sample_ok` wird false und die Fehlerstrategie der Thread-Gruppe greift; Continue fährt nach dem Controller fort. Benutzerabbruch und Gruppenlaufzeit behalten ihre bisherige Bedeutung.

Rust: `Node::LoopController(LoopController::new(name, count, children))` und `WhileController::timeout` verwenden standardmäßig `Duration::ZERO`; `Node::Loop` bleibt unbegrenzt. `ComponentNode::LoopController` enthält name/count/timeout/children; `ComponentNode::While` benötigt ebenfalls timeout. Direkte Cursor-Nutzer behandeln `CursorStep::Timeout`, begrenzen Sampler-Futures mit `timeout_remaining()` und ziehen Berichtswartezeiten mit `exclude_reporting_time()` ab.
