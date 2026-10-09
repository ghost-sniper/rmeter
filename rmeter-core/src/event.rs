use std::time::SystemTime;

use crate::{context::UserContext, sampler::SampleResult};

#[derive(Debug)]
pub struct SampleEvent {
    pub group_name: String,
    pub user_index: usize,
    pub iteration: u64,
    pub result: SampleResult,
}

impl SampleEvent {
    pub fn new(ctx: &UserContext, result: SampleResult) -> Self {
        Self {
            group_name: ctx.group_name.clone(),
            user_index: ctx.user_index,
            iteration: ctx.iteration,
            result,
        }
    }
}
#[derive(Debug)]
pub enum RunEvent {
    Started { at: SystemTime },
    SampleCompleted(SampleEvent),
    Finished { at: SystemTime },
}
