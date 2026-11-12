use std::time::Duration;

use crate::plan::{LoopCount, Node};

use super::{Controller, GenericController};

/// Named counted loop with an independent timeout budget for each iteration.
#[derive(Debug)]
pub struct LoopController<S> {
    pub generic: GenericController<S>,
    pub count: LoopCount,
    /// Zero disables the timeout. Includes all child processing and waiting.
    pub timeout: Duration,
}

impl<S> LoopController<S> {
    pub fn new(name: impl Into<String>, count: LoopCount, children: Vec<Node<S>>) -> Self {
        Self {
            generic: GenericController::new(name, children),
            count,
            timeout: Duration::ZERO,
        }
    }
}

impl<S> Controller<S> for LoopController<S> {
    fn generic(&self) -> &GenericController<S> {
        &self.generic
    }
}
