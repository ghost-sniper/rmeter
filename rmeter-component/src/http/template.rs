use std::collections::HashMap;

use super::HttpConfigError;

pub(super) fn expand(
    input: &str,
    variables: &HashMap<String, String>,
) -> Result<String, HttpConfigError> {
    substitute(input, |name| {
        variables
            .get(name)
            .cloned()
            .ok_or_else(|| HttpConfigError::new(format!("undefined variable {name:?}")))
    })
}

pub(super) fn validate(input: &str) -> Result<(), HttpConfigError> {
    substitute(input, |_| Ok(String::new())).map(|_| ())
}

fn substitute(
    input: &str,
    mut value: impl FnMut(&str) -> Result<String, HttpConfigError>,
) -> Result<String, HttpConfigError> {
    let mut output = String::new();
    let mut rest = input;
    while let Some(start) = rest.find("${") {
        output.push_str(&rest[..start]);
        rest = &rest[start + 2..];
        let end = rest
            .find('}')
            .ok_or_else(|| HttpConfigError::new("unterminated variable expression"))?;
        let name = &rest[..end];
        if name.is_empty() || name.contains(['{', '$']) {
            return Err(HttpConfigError::new("invalid variable expression"));
        }
        output.push_str(&value(name)?);
        rest = &rest[end + 1..];
    }
    output.push_str(rest);
    Ok(output)
}

pub(super) fn expand_json(
    value: &serde_json::Value,
    variables: &HashMap<String, String>,
) -> Result<serde_json::Value, HttpConfigError> {
    use serde_json::Value;
    Ok(match value {
        Value::String(text) => Value::String(expand(text, variables)?),
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| expand_json(value, variables))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| Ok((key.clone(), expand_json(value, variables)?)))
                .collect::<Result<_, HttpConfigError>>()?,
        ),
        other => other.clone(),
    })
}

pub(super) fn json_strings<'a>(value: &'a serde_json::Value, output: &mut Vec<&'a str>) {
    match value {
        serde_json::Value::String(text) => output.push(text),
        serde_json::Value::Array(values) => {
            for value in values {
                json_strings(value, output);
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values() {
                json_strings(value, output);
            }
        }
        _ => {}
    }
}
