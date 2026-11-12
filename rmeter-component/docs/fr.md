# rmeter-component

[English](crate) · [简体中文](crate::translations::zh_cn) · [繁體中文](crate::translations::zh_tw) · [日本語](crate::translations::ja) · **Français** · [Deutsch](crate::translations::de)

L’accueil de la crate et la référence API sont en anglais par défaut. Ces guides existent en six langues ; les identifiants Rust et les commentaires de référence API restent en anglais.

## Présentation

`rmeter-component` fournit à `rmeter-core` des échantillonneurs HTTP/HTTPS, PostgreSQL (avec des noms de composants de style JDBC) et JavaScript synchrone. Il inclut les assertions JSONPath, regex et XML, les extracteurs HTTP, les valeurs par défaut à portée lexicale et les temporisateurs constants/gaussiens. PostgreSQL utilise des pilotes Rust natifs, sans JVM ni pilote Java JDBC.

## Démarrage rapide

Exécutez l’exemple dans Tokio avec un service local renvoyant `{"ready":true}`. Les constructeurs valident la configuration statique ; la requête HTTP n’est envoyée que lorsque `sample` est attendu.

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

## Exécution et erreurs

[`ComponentCompiler`](crate::ComponentCompiler) résout les valeurs par défaut, les pools nommés et les temporisateurs en nœuds exécutables. Les valeurs internes remplacent les valeurs externes ; les réglages explicites de la requête remplacent les valeurs héritées. Les listes header/query explicites remplacent entièrement les listes héritées. Les clients HTTP compatibles et les pools nommés sont partagés, mais les variables utilisateur restent isolées. Les cookies HTTP ne sont pas conservés automatiquement.

L’ordre est prétraitements → temporisateurs → échantillonneur → post-traitements → assertions. Les traitements ne produisent pas d’événement séparé. Leur échec ou celui d’une assertion fait échouer l’échantillon parent. JavaScript ne valide les changements de variables qu’en cas de succès ; leur annulation ne défait pas les opérations externes en base. Les post-traitements lisent le résultat avant assertions via `prev`, en lecture seule.

Le corps conservé dans le rapport, l’entrée des assertions et celle des post-traitements ont des limites distinctes. Un corps tronqué n’est pas une entrée complète d’assertion. Les erreurs HTTP opérationnelles donnent un `SampleResult` en échec : vérifiez `success` même si `sample` retourne `Ok`.

## Référence API

- [`http`](crate::http) : requêtes, corps, valeurs par défaut et arbres HTTP.
- [`jdbc`](crate::jdbc) : pools PostgreSQL nommés et requêtes paramétrées.
- [`javascript`](crate::javascript) : scripts, limites et worker.
- [`assertion`](crate::assertion) / [`extractor`](crate::extractor) : vérifications et extraction de variables.
- [`component`](crate::component) / [`timer`](crate::timer) : arbres mixtes et délais.

## Worker JavaScript et extracteurs

Compilez le worker dans cet espace de travail et déployez l’exécutable de même version à côté de l’application hôte, ou configurez son chemin absolu de confiance via `rmeter_component::javascript::set_worker_executable` avant de compiler les plans (`rmeter::component` via la façade). L’hôte peut aussi définir `RMETER_JS_WORKER`. JavaScript, les traitements et les extracteurs HTTP natifs nécessitent ce worker ; les plans HTTP/JDBC sans ces fonctions n’en ont pas besoin. Le worker limite les ressources et isole les défaillances, sans constituer un bac à sable complet du système.

```sh
cargo build -p rmeter-component --release --bin rmeter-js-worker
```

## Fonctions natives

RandomString, changeCase, dateTimeConvert, Random et RandomDate sont partagées entre Rust, JavaScript (`functions`) et les expressions `${__function(...)}`. Les formats de date suivent les règles ISO/SMART du JDK 21 avec des données locales et de fuseaux intégrées, sans Java à l’exécution. Les valeurs système sont figées au début de chaque exécution. Les appels invalides échouent explicitement. Le nom de variable facultatif enregistre le résultat pour l’utilisateur courant ; un échec annule les écritures de l’expression. Le code JavaScript n’est jamais interpolé. L’évaluation des contrôleurs reçoit désormais un UserContext mutable pour écrire les variables.
