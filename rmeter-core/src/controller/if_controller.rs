use crate::{context::UserContext, plan::Node};

use super::{Condition, ConditionError, Controller, GenericController};

/// Enter children when a condition is true, optionally rechecking before samples.
#[derive(Debug)]
pub struct IfController<S> {
    pub generic: GenericController<S>,
    pub condition: Condition,
    /// Recheck before each sampler, including samplers inside nested controllers.
    /// False abandons the remaining subtree for this entry, not future entries.
    pub evaluate_all: bool,
}

impl<S> IfController<S> {
    pub fn new(
        name: impl Into<String>,
        condition: impl Into<Condition>,
        children: Vec<Node<S>>,
    ) -> Self {
        Self {
            generic: GenericController::new(name, children),
            condition: condition.into(),
            evaluate_all: false,
        }
    }
}

impl<S> Controller<S> for IfController<S> {
    fn generic(&self) -> &GenericController<S> {
        &self.generic
    }
    fn should_enter(&self, context: &UserContext) -> Result<bool, ConditionError> {
        self.condition.evaluate(context)
    }
    fn before_sample(&self, context: &UserContext) -> Result<bool, ConditionError> {
        if self.evaluate_all {
            self.condition.evaluate(context)
        } else {
            Ok(true)
        }
    }
}
