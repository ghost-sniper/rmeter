use regex::Regex;
use rmeter_core::{
    assertion::{Assertion, AssertionInput, AssertionResult},
    context::UserContext,
};

use super::{AssertionConfigError, body_text};

/// A search over the complete UTF-8 response body, without JSON/XML parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegexAssertionConfig {
    pub name: String,
    pub pattern: String,
    pub invert: bool,
}
impl RegexAssertionConfig {
    pub fn new(name: impl Into<String>, pattern: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            pattern: pattern.into(),
            invert: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct RegexAssertion {
    config: RegexAssertionConfig,
    regex: Regex,
}
impl RegexAssertion {
    pub fn new(config: RegexAssertionConfig) -> Result<Self, AssertionConfigError> {
        let regex = Regex::new(&config.pattern).map_err(|error| {
            AssertionConfigError::new("regex", &config.name, format!("invalid regex: {error}"))
        })?;
        Ok(Self { config, regex })
    }
    pub fn config(&self) -> &RegexAssertionConfig {
        &self.config
    }
    pub(super) fn evaluate_input(&self, input: &AssertionInput<'_>) -> AssertionResult {
        let text = match body_text(input) {
            Ok(text) => text,
            Err(error) => return AssertionResult::failed(&self.config.name, error),
        };
        if self.regex.is_match(text) != self.config.invert {
            AssertionResult::passed(&self.config.name)
        } else {
            AssertionResult::failed(
                &self.config.name,
                format!(
                    "response body {} regex {:?}",
                    if self.config.invert {
                        "must not match"
                    } else {
                        "does not match"
                    },
                    super::shorten(&self.config.pattern),
                ),
            )
        }
    }
}
impl Assertion for RegexAssertion {
    fn evaluate(&self, input: &AssertionInput<'_>, _context: &UserContext) -> AssertionResult {
        self.evaluate_input(input)
    }
}
