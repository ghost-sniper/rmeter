use std::time::SystemTime;

use crate::{context::UserContext, sampler::SampleResult};

#[derive(Debug)]
pub struct SampleEvent {
    pub group_name: String,
    pub user_index: usize,
    pub iteration: u64,
    pub result: SampleResult,
    pub script_logs: crate::context::ScriptLogs,
}

impl SampleEvent {
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
#[derive(Debug)]
pub enum RunEvent {
    Started { at: SystemTime },
    SampleCompleted(Box<SampleEvent>),
    Finished { at: SystemTime },
}
