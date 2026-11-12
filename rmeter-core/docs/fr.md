# rmeter-core

[English](crate) · [简体中文](crate::translations::zh_cn) · [繁體中文](crate::translations::zh_tw) · [日本語](crate::translations::ja) · **Français** · [Deutsch](crate::translations::de)

L’accueil de la crate et la référence API sont en anglais par défaut. Ces guides existent en six langues ; les identifiants Rust et les commentaires de référence API restent en anglais.

## Présentation

`rmeter-core` définit les plans indépendants du protocole, l’état des utilisateurs virtuels, les traits d’échantillonnage et d’assertion, les contrôleurs et le moteur Tokio. Utilisez cette crate pour un échantillonneur personnalisé ou un plan typé, `rmeter` pour charger du JSON et `rmeter-component` pour les transports intégrés.

## Démarrage rapide

Cet exemple autonome parcourt un plan conditionnel sans réseau. `Node::Sample` peut contenir toute valeur pendant le parcours ; l’exécution par `Engine` exige un type implémentant `Sampler`.

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

## Exécution et erreurs

Créez un [`TestPlan`](crate::plan::TestPlan) contenant des [`ThreadGroup`](crate::plan::ThreadGroup), puis appelez [`Engine::run`](crate::engine::Engine::run) dans Tokio en consommant simultanément le canal borné. Chaque utilisateur possède ses variables et son curseur. Les indices utilisateur et itération du groupe commencent à zéro ; les variables persistent entre les échantillons et itérations du même utilisateur.

[`ErrorPolicy`](crate::plan::ErrorPolicy) choisit de continuer, passer à l’itération suivante, arrêter l’utilisateur ou arrêter le test après un échec. Les erreurs d’infrastructure et d’évaluation des conditions produisent [`EngineError`](crate::engine::EngineError). Une assertion réussie ne répare pas un échantillon en échec. Abandonner la future interrompt les tâches utilisateur.

Les expressions IF n’acceptent que `true`, sans distinction de casse. Les conditions While textuelles s’arrêtent sur `false` ; la chaîne vide et `LAST` ont une sémantique particulière liée au dernier échantillon. Avec [`ExecutionCursor`](crate::controller::ExecutionCursor), rendez la main au runtime sur `CursorStep::Yield` pour permettre l’annulation des boucles infinies sans échantillons.

## Référence API

- [`plan`](crate::plan) : définitions immuables et politiques d’erreur.
- [`sampler`](crate::sampler) / [`assertion`](crate::assertion) : traits d’extension et résultats.
- [`context`](crate::context) : variables utilisateur et journaux bornés.
- [`controller`](crate::controller) : séquences, IF, While et parcours.
- [`engine`](crate::engine) / [`event`](crate::event) : exécution concurrente et événements.
