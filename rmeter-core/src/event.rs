//! Engine lifecycle events and samples annotated with virtual-user identity.

use std::time::SystemTime;

use crate::{context::UserContext, sampler::SampleResult};

/// A completed sample with zero-based user and thread-group iteration indices.
#[derive(Debug)]
pub struct SampleEvent {
    pub group_name: String,
    pub user_index: usize,
    pub iteration: u64,
    pub result: SampleResult,
    pub script_logs: crate::context::ScriptLogs,
}

impl SampleEvent {
    /// Snapshot user identity and script logs alongside the completed result.
    pub fn new(ctx: &UserContext, result: SampleResult) -> Self {
        Self {
            group_name: ctx.group_name.clone(),
            user_index: ctx.user_index,
            iteration: ctx.iteration,
            result,
            script_logs: ctx.script_logs.clone(),
        }
    }
}
/// Lifecycle notifications and samples delivered through the event channel.
/// `Finished` indicates teardown, not that all samples or engine work succeeded.
#[derive(Debug)]
pub enum RunEvent {
    Started { at: SystemTime },
    SampleCompleted(Box<SampleEvent>),
    Finished { at: SystemTime },
}
