use std::fmt;

use regex::Regex;
use rmeter_core::{
    assertion::{Assertion, AssertionInput, AssertionResult},
    context::UserContext,
};
use serde_json::Value;
use serde_json_path::JsonPath;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MatchSelection {
    /// Require exactly one node. Never silently select the first match.
    #[default]
    One,
    Any,
    /// Require a nonempty result set, and every selected node must match.
    All,
}

#[derive(Debug, Clone, PartialEq)]
pub enum JsonMatcher {
    /// serde_json::Value equality, preserving JSON types and array ordering.
    Equals(Value),
    /// Search string nodes only; use ^...$ for an entire-string match.
    Regex(String),
    /// At least one selected node; JSON null counts as an existing value.
    Exists,
    NotExists,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JsonAssertionConfig {
    pub name: String,
    pub path: String,
    pub matcher: JsonMatcher,
    pub selection: MatchSelection,
}

impl JsonAssertionConfig {
    pub fn equals(name: impl Into<String>, path: impl Into<String>, expected: Value) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
            matcher: JsonMatcher::Equals(expected),
            selection: MatchSelection::One,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonAssertionError {
    pub name: String,
    pub message: String,
}

impl fmt::Display for JsonAssertionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JSON assertion {:?}: {}", self.name, self.message)
    }
}
impl std::error::Error for JsonAssertionError {}

/// Parsed JSONPath and regex are immutable and compiled once per configuration.
#[derive(Debug, Clone)]
pub struct JsonAssertion {
    config: JsonAssertionConfig,
    path: JsonPath,
    regex: Option<Regex>,
}

impl JsonAssertion {
    pub fn new(config: JsonAssertionConfig) -> Result<Self, JsonAssertionError> {
        let error = |message| JsonAssertionError {
            name: config.name.clone(),
            message,
        };
        let path = JsonPath::parse(&config.path)
            .map_err(|source| error(format!("invalid JSONPath {:?}: {source}", config.path)))?;
        let regex = match &config.matcher {
            JsonMatcher::Regex(pattern) => Some(
                Regex::new(pattern).map_err(|source| error(format!("invalid regex: {source}")))?,
            ),
            _ => None,
        };
        Ok(Self {
            config,
            path,
            regex,
        })
    }

    pub fn config(&self) -> &JsonAssertionConfig {
        &self.config
    }

    pub(super) fn failure(&self, message: impl Into<String>) -> AssertionResult {
        AssertionResult::failed(
            &self.config.name,
            format!("JSONPath {:?}: {}", self.config.path, message.into()),
        )
    }

    pub fn evaluate_json(&self, document: &Value) -> AssertionResult {
        let nodes = self.path.query(document).all();
        let matched = match &self.config.matcher {
            JsonMatcher::Exists => !nodes.is_empty(),
            JsonMatcher::NotExists => nodes.is_empty(),
            matcher => {
                if nodes.is_empty() {
                    return self.failure("no matching node");
                }
                if self.config.selection == MatchSelection::One && nodes.len() != 1 {
                    return self
                        .failure(format!("expected exactly one node, found {}", nodes.len()));
                }
                let matches = |value: &&Value| match matcher {
                    JsonMatcher::Equals(expected) => *value == expected,
                    JsonMatcher::Regex(_) => value
                        .as_str()
                        .is_some_and(|text| self.regex.as_ref().unwrap().is_match(text)),
                    _ => unreachable!(),
                };
                match self.config.selection {
                    MatchSelection::One | MatchSelection::All => nodes.iter().all(matches),
                    MatchSelection::Any => nodes.iter().any(matches),
                }
            }
        };
        if matched {
            return AssertionResult::passed(&self.config.name);
        }
        let expected = match &self.config.matcher {
            JsonMatcher::Equals(value) => format!("equals {}", describe(value)),
            JsonMatcher::Regex(pattern) => format!("string matching regex {:?}", shorten(pattern)),
            JsonMatcher::Exists => "an existing node".into(),
            JsonMatcher::NotExists => "no matching node".into(),
        };
        let actual = nodes
            .iter()
            .take(3)
            .map(|value| describe(value))
            .collect::<Vec<_>>()
            .join(", ");
        self.failure(format!(
            "expected {expected} ({:?}); found {} node(s): [{actual}{}]",
            self.config.selection,
            nodes.len(),
            if nodes.len() > 3 { ", ..." } else { "" }
        ))
    }
}

impl Assertion for JsonAssertion {
    fn evaluate(&self, input: &AssertionInput<'_>, _context: &UserContext) -> AssertionResult {
        match parse_body(input) {
            Ok(document) => self.evaluate_json(&document),
            Err(message) => self.failure(message),
        }
    }
}

pub(super) fn parse_body(input: &AssertionInput<'_>) -> Result<Value, String> {
    let body = super::body_bytes(input)?;
    serde_json::from_slice(body).map_err(|error| format!("invalid JSON response: {error}"))
}

fn shorten(text: &str) -> String {
    let mut chars = text.chars();
    let mut output: String = chars.by_ref().take(160).collect();
    if chars.next().is_some() {
        output.push_str("...");
    }
    output
}

fn describe(value: &Value) -> String {
    match value {
        Value::String(text) => format!("{:?}", shorten(text)),
        Value::Array(values) => format!("array({} elements)", values.len()),
        Value::Object(values) => format!("object({} properties)", values.len()),
        other => other.to_string(),
    }
}
