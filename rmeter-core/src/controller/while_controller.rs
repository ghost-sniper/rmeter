use crate::{context::UserContext, plan::Node};

use super::{Condition, ConditionError, Controller, GenericController, condition::expand};

/// A structured condition or string with JMeter-style While semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WhileCondition {
    Condition(Condition),
    /// JMeter semantics: blank enters unconditionally, then checks the final
    /// sample; LAST checks the previous sample on entry and at iteration end;
    /// other strings continue unless they expand to "false" (ignoring case).
    Expression(String),
}

impl WhileCondition {
    fn evaluate(&self, context: &UserContext, loop_end: bool) -> Result<bool, ConditionError> {
        match self {
            Self::Condition(condition) => condition.evaluate(context),
            Self::Expression(expression) => {
                let expression = expand(expression, context)?;
                let expression = expression.trim();
                if expression.eq_ignore_ascii_case("LAST") || (loop_end && expression.is_empty()) {
                    Ok(context.last_sample_ok)
                } else {
                    Ok(!expression.eq_ignore_ascii_case("false"))
                }
            }
        }
    }
}

impl From<Condition> for WhileCondition {
    fn from(value: Condition) -> Self {
        Self::Condition(value)
    }
}
impl From<bool> for WhileCondition {
    fn from(value: bool) -> Self {
        Self::Condition(value.into())
    }
}
impl From<String> for WhileCondition {
    fn from(value: String) -> Self {
        Self::Expression(value)
    }
}
impl From<&str> for WhileCondition {
    fn from(value: &str) -> Self {
        Self::Expression(value.into())
    }
}

/// Repeat children while permitted, with a per-user iteration variable.
#[derive(Debug)]
pub struct WhileController<S> {
    pub generic: GenericController<S>,
    pub condition: WhileCondition,
}

impl<S> WhileController<S> {
    pub fn new(
        name: impl Into<String>,
        condition: impl Into<WhileCondition>,
        children: Vec<Node<S>>,
    ) -> Self {
        Self {
            generic: GenericController::new(name, children),
            condition: condition.into(),
        }
    }

    /// Zero-based per-user iteration variable, compatible with JMeter's naming.
    pub fn index_variable(&self) -> String {
        format!("__jm__{}__idx", self.generic.name)
    }
}

impl<S> Controller<S> for WhileController<S> {
    fn generic(&self) -> &GenericController<S> {
        &self.generic
    }
    fn should_enter(&self, context: &UserContext) -> Result<bool, ConditionError> {
        self.condition.evaluate(context, false)
    }
    fn should_repeat(&self, context: &UserContext) -> Result<bool, ConditionError> {
        self.condition.evaluate(context, true)
    }
}
