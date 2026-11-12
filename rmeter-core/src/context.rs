//! Per-user mutable variables, execution indices and bounded script diagnostics.

use std::collections::HashMap;

/// Mutable state belonging to one virtual user, retained across its iterations.
#[derive(Debug)]
pub struct UserContext {
    /// Locale and timezone captured at run start; shared with all function entry points.
    pub function_environment: rmeter_function::Environment,
    /// Group name attached to sample events.
    pub group_name: String,
    /// Zero-based virtual-user index within the group.
    pub user_index: usize,
    /// Zero-based thread-group iteration index.
    pub iteration: u64,
    /// String variables retained for this user across samples and iterations.
    pub variables: HashMap<String, String>,
    /// Final outcome of the previous sample; initially true.
    pub last_sample_ok: bool,
    /// Bounded diagnostics for the current sample's processing chain.
    pub script_logs: ScriptLogs,
}
impl UserContext {
    /// Create an empty context at iteration zero with `last_sample_ok = true`.
    pub fn new(group_name: impl Into<String>, user_index: usize) -> Self {
        Self {
            function_environment: rmeter_function::Environment::default(),
            group_name: group_name.into(),
            user_index,
            iteration: 0,
            variables: HashMap::new(),
            last_sample_ok: true,
            script_logs: ScriptLogs::default(),
        }
    }
}

/// Bounded diagnostics per sample, shared by its pre/main/post scripts.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ScriptLogs {
    pub entries: Vec<ScriptLog>,
    pub truncated: bool,
}
/// One script diagnostic, with its level and originating script name.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScriptLog {
    pub level: String,
    pub source: String,
    pub message: String,
}
impl ScriptLogs {
    /// Append within limits: 100 entries, 64 KiB total, and 4 KiB per message.
    /// UTF-8 boundaries are preserved; dropped or shortened logs set `truncated`.
    pub fn push(&mut self, mut entry: ScriptLog) {
        let used: usize = self
            .entries
            .iter()
            .map(|v| v.level.len() + v.source.len() + v.message.len())
            .sum();
        if entry.message.len() > 4096 {
            entry.message = entry
                .message
                .chars()
                .scan(0, |n, c| {
                    *n += c.len_utf8();
                    (*n <= 4096).then_some(c)
                })
                .collect();
            self.truncated = true;
        }
        if self.entries.len() >= 100
            || used + entry.level.len() + entry.source.len() + entry.message.len() > 65536
        {
            self.truncated = true;
        } else {
            self.entries.push(entry);
        }
    }
    /// Merge diagnostics, retaining truncation flags and enforcing limits.
    pub fn extend(&mut self, logs: Self) {
        self.truncated |= logs.truncated;
        for entry in logs.entries {
            self.push(entry);
        }
    }
}
