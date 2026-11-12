use rmeter_core::context::UserContext;

use super::HttpConfigError;

pub(crate) fn expand(input: &str, context: &mut UserContext) -> Result<String, HttpConfigError> {
    rmeter_function::Expression::cached(input)
        .and_then(|expression| {
            expression.evaluate(&mut rmeter_function::FunctionContext {
                variables: &mut context.variables,
                environment: &context.function_environment,
                source: &mut rmeter_function::SystemSource,
            })
        })
        .map_err(|error| HttpConfigError::new(error.to_string()))
}

pub(crate) fn validate(input: &str) -> Result<(), HttpConfigError> {
    rmeter_function::Expression::cached(input)
        .map(|_| ())
        .map_err(|e| HttpConfigError::new(e.to_string()))
}

pub(crate) fn expand_json(
    value: &serde_json::Value,
    context: &mut UserContext,
) -> Result<serde_json::Value, HttpConfigError> {
    use serde_json::Value;
    Ok(match value {
        Value::String(text) => Value::String(expand(text, context)?),
        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| expand_json(value, context))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(values) => Value::Object(
            values
                .iter()
                .map(|(key, value)| Ok((key.clone(), expand_json(value, context)?)))
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
