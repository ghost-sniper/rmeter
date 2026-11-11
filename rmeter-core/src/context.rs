use std::collections::HashMap;

#[derive(Debug)]
pub struct UserContext {
    pub group_name: String,
    pub user_index: usize,
    pub iteration: u64,
    pub variables: HashMap<String, String>,
    pub last_sample_ok: bool,
    pub script_logs: ScriptLogs,
}
impl UserContext {
    pub fn new(group_name: impl Into<String>, user_index: usize) -> Self {
        Self {
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ScriptLog {
    pub level: String,
    pub source: String,
    pub message: String,
}
impl ScriptLogs {
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
    pub fn extend(&mut self, logs: Self) {
        self.truncated |= logs.truncated;
        for entry in logs.entries {
            self.push(entry);
        }
    }
}
