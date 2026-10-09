use crate::{context::UserContext, sampler::SampleResult};

/// Per-assertion diagnostics, kept separately from transport/status information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssertionResult {
    pub name: String,
    pub success: bool,
    pub message: Option<String>,
}

impl AssertionResult {
    pub fn passed(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            success: true,
            message: None,
        }
    }

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

pub trait Assertion: Send + Sync {
    fn evaluate(&self, input: &AssertionInput<'_>, context: &UserContext) -> AssertionResult;
}
