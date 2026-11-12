use crate::{context::UserContext, plan::Node};

use super::ConditionError;

/// Shared immutable controller definition. Specialized controllers compose this
/// type rather than owning their own child traversal implementation.
#[derive(Debug)]
pub struct GenericController<S> {
    pub name: String,
    pub children: Vec<Node<S>>,
}

impl<S> GenericController<S> {
    pub fn new(name: impl Into<String>, children: Vec<Node<S>>) -> Self {
        Self {
            name: name.into(),
            children,
        }
    }
}

/// Lifecycle hooks analogous to JMeter's GenericController overrides.
/// Hooks are pure; state belongs to the user's execution cursor, not the plan.
pub trait Controller<S> {
    fn generic(&self) -> &GenericController<S>;
    fn should_enter(&self, _context: &mut UserContext) -> Result<bool, ConditionError> {
        Ok(true)
    }
    fn should_repeat(&self, _context: &mut UserContext) -> Result<bool, ConditionError> {
        Ok(false)
    }
    fn before_sample(&self, _context: &mut UserContext) -> Result<bool, ConditionError> {
        Ok(true)
    }
}

impl<S> Controller<S> for GenericController<S> {
    fn generic(&self) -> &GenericController<S> {
        self
    }
}

/// Common per-user traversal state, separate from shared controller definitions.
#[derive(Debug, Default)]
pub(super) struct GenericControllerState {
    pub current: usize,
    pub iteration: u64,
    pub first: bool,
    pub done: bool,
}

impl GenericControllerState {
    pub fn initialize() -> Self {
        Self {
            first: true,
            ..Self::default()
        }
    }

    pub fn reinitialize(&mut self) -> bool {
        let Some(iteration) = self.iteration.checked_add(1) else {
            self.done = true;
            return false;
        };
        self.current = 0;
        self.iteration = iteration;
        self.first = true;
        true
    }
}
