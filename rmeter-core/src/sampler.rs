//! Asynchronous sampler extension points and protocol-independent outcomes.
//!
//! Implement [`Sampler`] for a shareable definition and store user state in
//! [`UserContext`]. Returned futures must support cancellation by being dropped.

use std::time::{Duration, SystemTime};

use crate::{assertion::AssertionResult, context::UserContext};

/// Final sample outcome, timing, byte counters and individual assertion results.
#[derive(Debug)]
pub struct SampleResult {
    /// Sample label, normally with runtime variables already expanded.
    pub label: String,
    /// Wall-clock start of sampling.
    pub started_at: SystemTime,
    /// Duration measured by the sampler, excluding successful processing stages.
    pub elapsed: Duration,
    /// Final success after transport, processing and assertions.
    pub success: bool,
    /// Protocol status or component-specific error code, when available.
    pub response_code: Option<String>,
    /// Protocol message or failure diagnostic.
    pub response_message: Option<String>,
    /// Sent byte count as defined by the sampler, not necessarily wire traffic.
    pub bytes_sent: u64,
    /// Received byte count as defined by the sampler.
    pub bytes_received: u64,
    /// Optional saved response, possibly truncated by reporting limits.
    pub response_body: Option<Vec<u8>>,
    /// Individual assertion outcomes in evaluation order.
    pub assertions: Vec<AssertionResult>,
}

impl SampleResult {
    /// Assertion success can never turn a failed transport/status into success.
    pub fn add_assertion(&mut self, assertion: AssertionResult) {
        self.success &= assertion.success;
        self.assertions.push(assertion);
    }
}

/// Classification of a failure returned through the sampler error channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleErrorKind {
    InvalidConfig,
    Timeout,
    Transport,
    Cancelled,
    Internal,
}

/// A classified sampler failure with a human-readable diagnostic.
#[derive(Debug)]
pub struct SampleError {
    pub kind: SampleErrorKind,
    pub message: String,
}

/// A shareable asynchronous operation executed with an exclusive user context.
///
/// The engine can drop the future on cancellation. Avoid leaving detached work
/// running; keep mutable user-specific state in the supplied context.
pub trait Sampler: Send + Sync {
    /// Execute once, updating this user's state and returning its outcome.
    fn sample(
        &self,
        ctx: &mut UserContext,
    ) -> impl Future<Output = Result<SampleResult, SampleError>> + Send;
}
