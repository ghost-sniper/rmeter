use std::time::{Duration, SystemTime};

use crate::{assertion::AssertionResult, context::UserContext};

#[derive(Debug)]
pub struct SampleResult {
    pub label: String,
    pub started_at: SystemTime,
    pub elapsed: Duration,
    pub success: bool,
    pub response_code: Option<String>,
    pub response_message: Option<String>,
    pub bytes_sent: u64,
    pub bytes_received: u64,
    pub response_body: Option<Vec<u8>>,
    pub assertions: Vec<AssertionResult>,
}

impl SampleResult {
    /// Assertion success can never turn a failed transport/status into success.
    pub fn add_assertion(&mut self, assertion: AssertionResult) {
        self.success &= assertion.success;
        self.assertions.push(assertion);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleErrorKind {
    InvalidConfig,
    Timeout,
    Transport,
    Cancelled,
    Internal,
}

#[derive(Debug)]
pub struct SampleError {
    pub kind: SampleErrorKind,
    pub message: String,
}

pub trait Sampler: Send + Sync {
    fn sample(
        &self,
        ctx: &mut UserContext,
    ) -> impl Future<Output = Result<SampleResult, SampleError>> + Send;
}
