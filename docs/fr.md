# rmeter

[English](crate) · [简体中文](crate::translations::zh_cn) · [繁體中文](crate::translations::zh_tw) · [日本語](crate::translations::ja) · **Français** · [Deutsch](crate::translations::de)

L’accueil de la crate et la référence API sont en anglais par défaut. Ces guides existent en six langues ; les identifiants Rust et les commentaires de référence API restent en anglais.

## Présentation

`rmeter` est le point d’entrée par scripts de cet espace de travail de tests de charge Rust/Tokio. Il analyse les plans JSON de version 1, compile les composants HTTP, PostgreSQL et JavaScript, puis transmet les événements d’exécution. La compilation produit une bibliothèque par défaut ; la feature facultative `cli` active l’outil en ligne de commande.

## Démarrage rapide

Exécutez cet exemple dans un runtime Tokio existant, avec un service HTTP à l’adresse indiquée. Consommez les événements en parallèle : un canal borné plein suspend le moteur. Ajoutez `rmeter` comme dépendance Cargo par chemin local, ainsi que `tokio` et `serde_json` pour cet exemple.

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

## Exécution et erreurs

[`parse_script`](crate::parse_script) valide et compile sans requête HTTP ni connexion PostgreSQL. JavaScript et les extracteurs HTTP natifs nécessitent toutefois le worker séparé dès la validation. Le JSON UTF-8 peut commencer par un BOM. Les champs inconnus et les versions non prises en charge sont rejetés. Les variables sont des chaînes isolées par utilisateur virtuel.

[`run_script`](crate::run_script) et [`run_script_file`](crate::run_script_file) utilisent le runtime appelant. Abandonner la future en cours annule les utilisateurs virtuels. Un échec d’échantillon est signalé par événement et peut laisser le résultat du moteur à `Ok(())` : vérifiez `success`, [`RunSummary`](crate::report::RunSummary) et le résultat retourné. `Finished` seul ne garantit pas le succès. Les indices commencent à zéro et les horodatages sérialisés sont en millisecondes Unix.

## Référence API

- [`script`](crate::script) : analyse, lecture de fichiers et [`ScriptError`](crate::ScriptError).
- [`report`](crate::report) : événements sérialisables et compteurs.
- [`core`](crate::core) : plans, contrôleurs, contextes et moteur.
- [`component`](crate::component) : échantillonneurs HTTP/JDBC/JavaScript, assertions, extracteurs et temporisateurs.

## Worker JavaScript et extracteurs

Compilez le worker dans cet espace de travail et déployez l’exécutable de même version à côté de l’application hôte, ou configurez son chemin absolu de confiance via `rmeter_component::javascript::set_worker_executable` avant de compiler les plans (`rmeter::component` via la façade). L’hôte peut aussi définir `RMETER_JS_WORKER`. JavaScript, les traitements et les extracteurs HTTP natifs nécessitent ce worker ; les plans HTTP/JDBC sans ces fonctions n’en ont pas besoin. Le worker limite les ressources et isole les défaillances, sans constituer un bac à sable complet du système.

```sh
cargo build -p rmeter-component --release --bin rmeter-js-worker
```

## CLI et documentation

```sh
cargo run -p rmeter --features cli -- examples/http.json --check
cargo run -p rmeter --features cli -- examples/http.json
cargo doc --workspace --no-deps --open
cargo test --workspace --doc
```
