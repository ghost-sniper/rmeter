//! Transport-independent assertions evaluated against complete response input.
//!
//! Assertion input is separate from the potentially truncated saved response.

use crate::{context::UserContext, sampler::SampleResult};

/// Per-assertion diagnostics, kept separately from transport/status information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertionResult {
    pub name: String,
    pub success: bool,
    pub message: Option<String>,
}

impl AssertionResult {
    /// Create a passing result without a diagnostic message.
    pub fn passed(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            success: true,
            message: None,
        }
    }

    /// Create a failed result with a diagnostic message.
    pub fn failed(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            success: false,
            message: Some(message.into()),
        }
    }
}

/// The body is the complete assertion input, independent of a possibly truncated
/// saved response body. None indicates unavailable/incomplete/oversized input.
pub struct AssertionInput<'a> {
    pub sample: &'a SampleResult,
    pub response_body: Option<&'a [u8]>,
    pub unavailable_reason: Option<&'a str>,
}

/// A shareable response check that does not mutate user context.
pub trait Assertion: Send + Sync {
    /// Evaluate one complete input and return a named result.
    fn evaluate(&self, input: &AssertionInput<'_>, context: &UserContext) -> AssertionResult;
}
