use std::collections::{HashMap, HashSet};

use serde_json::{Map, Value};

use super::{JdbcConfigError, JdbcSamplerConfig};

fn row_index(key: &str, name: &str) -> Option<usize> {
    let suffix = key.strip_prefix(name)?.strip_prefix('_')?;
    let index: usize = suffix.parse().ok()?;
    (index > 0 && index.to_string() == suffix).then_some(index)
}

pub(super) fn validate(config: &mut JdbcSamplerConfig) -> Result<(), JdbcConfigError> {
    let mut columns = HashSet::new();
    for name in &mut config.variable_names {
        *name = name.trim().to_owned();
        if !name.is_empty() && !columns.insert(name.clone()) {
            return Err(JdbcConfigError::new("duplicate JDBC column variable name"));
        }
    }
    let mut fixed = Vec::new();
    if let Some(name) = &config.result_variable {
        fixed.extend([name.clone(), format!("{name}_#")]);
    }
    if let Some(name) = &config.row_count_variable {
        fixed.push(name.clone());
    }
    let mut seen = HashSet::new();
    for key in fixed {
        if !seen.insert(key.clone())
            || columns
                .iter()
                .any(|name| key == format!("{name}_#") || row_index(&key, name).is_some())
        {
            return Err(JdbcConfigError::new("JDBC result variable names overlap"));
        }
    }
    Ok(())
}

pub(super) fn clear(config: &JdbcSamplerConfig, variables: &mut HashMap<String, String>) {
    for name in config.variable_names.iter().filter(|name| !name.is_empty()) {
        let count_key = format!("{name}_#");
        if let Some(count) = variables
            .remove(&count_key)
            .and_then(|value| value.parse::<usize>().ok())
        {
            // Scan existing keys instead of looping up to a user-editable count.
            variables.retain(|key, _| !row_index(key, name).is_some_and(|index| index <= count));
        }
    }
    if let Some(name) = &config.result_variable {
        variables.remove(name);
        variables.remove(&format!("{name}_#"));
    }
    if let Some(name) = &config.row_count_variable {
        variables.remove(name);
    }
}

pub(super) fn export(
    config: &JdbcSamplerConfig,
    columns: &[String],
    rows: &[Vec<Value>],
) -> Result<HashMap<String, String>, String> {
    let mut variables = HashMap::new();
    let mut bytes = 0usize;
    let mut put = |key: String, value: String| -> Result<(), String> {
        bytes = bytes.saturating_add(key.len()).saturating_add(value.len());
        if bytes > config.max_result_bytes {
            return Err("JDBC exported variable byte limit exceeded".into());
        }
        variables.insert(key, value);
        Ok(())
    };
    let count = rows.len().to_string();
    if let Some(name) = &config.result_variable {
        let mut array = String::from("[");
        for (index, row) in rows.iter().enumerate() {
            let object: Map<String, Value> =
                columns.iter().cloned().zip(row.iter().cloned()).collect();
            let item = Value::Object(object).to_string();
            if array.len().saturating_add(item.len()).saturating_add(2) > config.max_result_bytes {
                return Err("JDBC result array byte limit exceeded".into());
            }
            if index > 0 {
                array.push(',');
            }
            array.push_str(&item);
        }
        array.push(']');
        put(name.clone(), array)?;
        put(format!("{name}_#"), count.clone())?;
    }
    for (column, name) in config
        .variable_names
        .iter()
        .enumerate()
        .filter(|(_, name)| !name.is_empty())
    {
        put(format!("{name}_#"), count.clone())?;
        for (index, row) in rows.iter().enumerate() {
            // NULL has no string variable, matching JMeter's null-valued lookup.
            if let Some(value) = row.get(column).filter(|value| !value.is_null()) {
                put(
                    format!("{name}_{}", index + 1),
                    match value {
                        Value::String(text) => text.clone(),
                        value => value.to_string(),
                    },
                )?;
            }
        }
    }
    if let Some(name) = &config.row_count_variable {
        put(name.clone(), count)?;
    }
    Ok(variables)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config() -> JdbcSamplerConfig {
        let mut config = JdbcSamplerConfig::new("query", "db", "SELECT 1");
        config.result_variable = Some("result".into());
        config.variable_names = vec!["id".into(), "".into(), "name".into(), "extra".into()];
        config.row_count_variable = Some("count".into());
        config
    }

    #[test]
    fn arrays_column_positions_nulls_and_zero_rows() {
        let config = config();
        let columns = vec!["id".into(), "enabled".into(), "name".into()];
        let rows = vec![
            vec![json!(1), json!(true), json!("中文")],
            vec![json!(2), json!(false), Value::Null],
        ];
        let variables = export(&config, &columns, &rows).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&variables["result"]).unwrap(),
            json!([{"id":1,"enabled":true,"name":"中文"},{"id":2,"enabled":false,"name":null}])
        );
        for key in ["result_#", "id_#", "name_#", "extra_#", "count"] {
            assert_eq!(variables[key], "2");
        }
        assert_eq!(variables["id_2"], "2");
        assert_eq!(variables["name_1"], "中文");
        assert!(!variables.contains_key("name_2"));
        assert!(!variables.contains_key("extra_1"));
        assert!(!variables.contains_key("_1"));
        let empty = export(&config, &columns, &[]).unwrap();
        assert_eq!(empty["result"], "[]");
        assert_eq!(empty["id_#"], "0");
        assert!(!empty.contains_key("id_1"));
        let mut only_array = config.clone();
        only_array.variable_names.clear();
        only_array.row_count_variable = None;
        let array = export(&only_array, &columns, &rows).unwrap();
        assert_eq!(array.len(), 2);
        assert!(array.contains_key("result"));
        let mut only_columns = config.clone();
        only_columns.result_variable = None;
        let indexed = export(&only_columns, &columns, &rows).unwrap();
        assert_eq!(indexed["id_1"], "1");
        assert!(!indexed.contains_key("result"));
    }

    #[test]
    fn clears_previous_results_without_unbounded_count_iteration_or_unrelated_keys() {
        let mut variables = HashMap::from([
            ("id_#".into(), usize::MAX.to_string()),
            ("id_1".into(), "stale".into()),
            ("id_9".into(), "stale".into()),
            ("id_01".into(), "keep".into()),
            ("id_suffix".into(), "keep".into()),
            ("result".into(), "stale".into()),
            ("result_#".into(), "9".into()),
            ("count".into(), "9".into()),
        ]);
        clear(&config(), &mut variables);
        assert_eq!(
            variables,
            HashMap::from([
                ("id_01".into(), "keep".into()),
                ("id_suffix".into(), "keep".into())
            ])
        );
    }

    #[test]
    fn validates_collisions_and_bounds_all_exported_variables() {
        for names in [vec!["id", "id"], vec![" result "]] {
            let mut config = config();
            config.variable_names = names.into_iter().map(str::to_owned).collect();
            assert!(validate(&mut config).is_err());
        }
        let mut config = config();
        config.result_variable = Some("id_1".into());
        assert!(validate(&mut config).is_err());
        config.result_variable = Some("result".into());
        config.variable_names = vec![" id ".into(), " ".into()];
        validate(&mut config).unwrap();
        assert_eq!(config.variable_names, vec!["id", ""]);
        config.max_result_bytes = 10;
        assert!(export(&config, &["id".into()], &[vec![json!(12345)]]).is_err());
    }
}
