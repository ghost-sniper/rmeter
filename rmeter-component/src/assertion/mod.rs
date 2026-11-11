//! Response assertions, independent of transport components.

mod json;
mod regex;
pub(crate) mod xml;

pub use json::{
    JsonAssertion, JsonAssertionConfig, JsonAssertionError, JsonMatcher, MatchSelection,
};
pub use regex::{RegexAssertion, RegexAssertionConfig};
pub use xml::{XmlAssertion, XmlAssertionConfig, XmlMatcher};

use rmeter_core::{
    assertion::{Assertion, AssertionInput, AssertionResult},
    context::UserContext,
};
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum AssertionConfig {
    Json(JsonAssertionConfig),
    Regex(RegexAssertionConfig),
    Xml(XmlAssertionConfig),
}
impl From<JsonAssertionConfig> for AssertionConfig {
    fn from(value: JsonAssertionConfig) -> Self {
        Self::Json(value)
    }
}
impl From<RegexAssertionConfig> for AssertionConfig {
    fn from(value: RegexAssertionConfig) -> Self {
        Self::Regex(value)
    }
}
impl From<XmlAssertionConfig> for AssertionConfig {
    fn from(value: XmlAssertionConfig) -> Self {
        Self::Xml(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertionConfigError {
    pub component: String,
    pub name: String,
    pub message: String,
}
impl AssertionConfigError {
    fn new(component: &str, name: &str, message: impl Into<String>) -> Self {
        Self {
            component: component.into(),
            name: name.into(),
            message: message.into(),
        }
    }
}
impl fmt::Display for AssertionConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} assertion {:?}: {}",
            self.component, self.name, self.message
        )
    }
}
impl std::error::Error for AssertionConfigError {}

#[derive(Debug, Clone)]
pub enum ResponseAssertion {
    Json(JsonAssertion),
    Regex(RegexAssertion),
    Xml(XmlAssertion),
}
impl ResponseAssertion {
    pub fn new(config: AssertionConfig) -> Result<Self, AssertionConfigError> {
        match config {
            AssertionConfig::Json(config) => JsonAssertion::new(config)
                .map(Self::Json)
                .map_err(|error| AssertionConfigError::new("JSON", &error.name, error.message)),
            AssertionConfig::Regex(config) => RegexAssertion::new(config).map(Self::Regex),
            AssertionConfig::Xml(config) => XmlAssertion::new(config).map(Self::Xml),
        }
    }
}
impl Assertion for ResponseAssertion {
    fn evaluate(&self, input: &AssertionInput<'_>, _context: &UserContext) -> AssertionResult {
        evaluate_assertions(std::slice::from_ref(self), input).remove(0)
    }
}

/// Preserve declaration order, evaluate every component, and lazily parse each
/// document format at most once per response, including cached parse failures.
pub(crate) fn evaluate_assertions(
    assertions: &[ResponseAssertion],
    input: &AssertionInput<'_>,
) -> Vec<AssertionResult> {
    let mut json = None;
    let mut xml = None;
    assertions
        .iter()
        .map(|assertion| match assertion {
            ResponseAssertion::Json(assertion) => {
                match json.get_or_insert_with(|| json::parse_body(input)) {
                    Ok(document) => assertion.evaluate_json(document),
                    Err(error) => assertion.failure(&*error),
                }
            }
            ResponseAssertion::Regex(assertion) => assertion.evaluate_input(input),
            ResponseAssertion::Xml(assertion) => {
                match xml.get_or_insert_with(|| xml::parse_body(input)) {
                    Ok(document) => assertion.evaluate_document(document),
                    Err(error) => assertion.failure(&*error),
                }
            }
        })
        .collect()
}

fn body_bytes<'a>(input: &AssertionInput<'a>) -> Result<&'a [u8], String> {
    if let Some(reason) = input.unavailable_reason {
        return Err(reason.into());
    }
    input
        .response_body
        .ok_or_else(|| "complete response body is unavailable".into())
}
fn body_text<'a>(input: &AssertionInput<'a>) -> Result<&'a str, String> {
    std::str::from_utf8(body_bytes(input)?)
        .map_err(|error| format!("response body is not UTF-8: {error}"))
}
fn shorten(text: &str) -> String {
    let mut chars = text.chars();
    let mut output: String = chars.by_ref().take(160).collect();
    if chars.next().is_some() {
        output.push_str("...");
    }
    output
}
