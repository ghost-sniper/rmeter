use std::collections::HashMap;

#[derive(Debug)]
pub struct UserContext {
    pub group_name: String,
    pub user_index: usize,
    pub iteration: u64,
    pub variables: HashMap<String, String>,
    pub last_sample_ok: bool,
}
impl UserContext {
    pub fn new(group_name: impl Into<String>, user_index: usize) -> Self {
        Self {
            group_name: group_name.into(),
            user_index,
            iteration: 0,
            variables: HashMap::new(),
            last_sample_ok: true,
        }
    }
}
