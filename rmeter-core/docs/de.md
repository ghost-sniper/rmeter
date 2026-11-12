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
