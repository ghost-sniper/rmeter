# rmeter-function

## Fonctions natives

RandomString, changeCase, dateTimeConvert, Random et RandomDate sont partagées entre Rust, JavaScript (`functions`) et les expressions `${__function(...)}`. Les formats de date suivent les règles ISO/SMART du JDK 21 avec des données locales et de fuseaux intégrées, sans Java à l’exécution. Les valeurs système sont figées au début de chaque exécution. Les appels invalides échouent explicitement. Le nom de variable facultatif enregistre le résultat pour l’utilisateur courant ; un échec annule les écritures de l’expression. Le code JavaScript n’est jamais interpolé. L’évaluation des contrôleurs reçoit désormais un UserContext mutable pour écrire les variables.

```rust
use rmeter_function::{date_time_convert, Environment};
let env = Environment { timezone: "UTC".into(), locale: "en-US".into() };
assert_eq!(date_time_convert("2024-02-29", "yyyy-MM-dd", "dd/MM/yyyy", &env).unwrap(), "29/02/2024");
```
