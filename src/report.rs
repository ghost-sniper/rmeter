use std::time::{SystemTime, UNIX_EPOCH};

use rmeter_core::event::RunEvent;
use serde::{Deserialize, Serialize};

/// JSON-friendly events for CLI output and a future Tauri event/channel adapter.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReportEvent {
    Started {
        at_ms: u64,
    },
    SampleCompleted {
        group_name: String,
        user_index: usize,
        iteration: u64,
        label: String,
        started_at_ms: u64,
        elapsed_ms: f64,
        success: bool,
        response_code: Option<String>,
        response_message: Option<String>,
        bytes_sent: u64,
        bytes_received: u64,
        response_body: Option<Vec<u8>>,
        assertions: Vec<AssertionReport>,
    },
    Finished {
        at_ms: u64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssertionReport {
    pub name: String,
    pub success: bool,
    pub message: Option<String>,
}

impl From<RunEvent> for ReportEvent {
    fn from(event: RunEvent) -> Self {
        match event {
            RunEvent::Started { at } => Self::Started { at_ms: millis(at) },
            RunEvent::Finished { at } => Self::Finished { at_ms: millis(at) },
            RunEvent::SampleCompleted(event) => {
                let result = event.result;
                Self::SampleCompleted {
                    group_name: event.group_name,
                    user_index: event.user_index,
                    iteration: event.iteration,
                    label: result.label,
                    started_at_ms: millis(result.started_at),
                    elapsed_ms: result.elapsed.as_secs_f64() * 1000.0,
                    success: result.success,
                    response_code: result.response_code,
                    response_message: result.response_message,
                    bytes_sent: result.bytes_sent,
                    bytes_received: result.bytes_received,
                    response_body: result.response_body,
                    assertions: result
                        .assertions
                        .into_iter()
                        .map(|assertion| AssertionReport {
                            name: assertion.name,
                            success: assertion.success,
                            message: assertion.message,
                        })
                        .collect(),
                }
            }
        }
    }
}
fn millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RunSummary {
    pub samples: u64,
    pub passed: u64,
    pub failed: u64,
    /// True only if the engine emitted Finished; inspect the engine Result too.
    pub finished: bool,
}
impl RunSummary {
    pub fn observe(&mut self, event: &RunEvent) {
        match event {
            RunEvent::SampleCompleted(sample) => {
                self.samples += 1;
                if sample.result.success {
                    self.passed += 1;
                } else {
                    self.failed += 1;
                }
            }
            RunEvent::Finished { .. } => self.finished = true,
            RunEvent::Started { .. } => {}
        }
    }
}
