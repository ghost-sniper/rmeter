use std::{cmp::Ordering, fmt};

use crate::context::UserContext;

/// Pure conditions evaluated against the current virtual user's context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    Constant(bool),
    VariableBool(String),
    LastSampleOk,
    /// JMeter variable-expression mode: only "true" (ignoring case) is true.
    /// Supports ${variable}, ${iteration}, ${user_index}, ${last_sample_ok},
    /// and ${JMeterThread.last_sample_ok}. Does not execute JavaScript/Groovy.
    Expression(String),
    Compare {
        left: Operand,
        operator: CompareOperator,
        right: Operand,
    },
    Not(Box<Condition>),
    All(Vec<Condition>),
    Any(Vec<Condition>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Operand {
    Literal(String),
    Variable(String),
    Iteration,
    UserIndex,
    LastSampleOk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOperator {
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionError {
    pub message: String,
}

impl ConditionError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for ConditionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for ConditionError {}

impl Condition {
    pub fn evaluate(&self, context: &UserContext) -> Result<bool, ConditionError> {
        Ok(match self {
            Self::Constant(value) => *value,
            Self::VariableBool(name) => {
                let value = variable(name, context)?;
                if value.eq_ignore_ascii_case("true") {
                    true
                } else if value.eq_ignore_ascii_case("false") {
                    false
                } else {
                    return Err(ConditionError::new(format!(
                        "variable {name:?} must be true or false"
                    )));
                }
            }
            Self::LastSampleOk => context.last_sample_ok,
            Self::Expression(expression) => expand(expression, context)?
                .trim()
                .eq_ignore_ascii_case("true"),
            Self::Compare {
                left,
                operator,
                right,
            } => {
                let left = left.value(context)?;
                let right = right.value(context)?;
                match operator {
                    CompareOperator::Equal => left == right,
                    CompareOperator::NotEqual => left != right,
                    operator => {
                        // Exact signed integers; avoid losing u64 iteration precision
                        // through floating point conversion.
                        let number = |text: &str| {
                            text.parse::<i128>().map_err(|_| {
                                ConditionError::new(format!("{text:?} is not an integer"))
                            })
                        };
                        let order = number(&left)?.cmp(&number(&right)?);
                        match operator {
                            CompareOperator::Less => order == Ordering::Less,
                            CompareOperator::LessOrEqual => order != Ordering::Greater,
                            CompareOperator::Greater => order == Ordering::Greater,
                            CompareOperator::GreaterOrEqual => order != Ordering::Less,
                            _ => unreachable!(),
                        }
                    }
                }
            }
            Self::Not(condition) => !condition.evaluate(context)?,
            Self::All(conditions) => {
                for condition in conditions {
                    if !condition.evaluate(context)? {
                        return Ok(false);
                    }
                }
                true
            }
            Self::Any(conditions) => {
                for condition in conditions {
                    if condition.evaluate(context)? {
                        return Ok(true);
                    }
                }
                false
            }
        })
    }
}

impl From<bool> for Condition {
    fn from(value: bool) -> Self {
        Self::Constant(value)
    }
}
impl From<String> for Condition {
    fn from(value: String) -> Self {
        Self::Expression(value)
    }
}
impl From<&str> for Condition {
    fn from(value: &str) -> Self {
        Self::Expression(value.into())
    }
}

impl Operand {
    fn value(&self, context: &UserContext) -> Result<String, ConditionError> {
        Ok(match self {
            Self::Literal(value) => value.clone(),
            Self::Variable(name) => variable(name, context)?,
            Self::Iteration => context.iteration.to_string(),
            Self::UserIndex => context.user_index.to_string(),
            Self::LastSampleOk => context.last_sample_ok.to_string(),
        })
    }
}

fn variable(name: &str, context: &UserContext) -> Result<String, ConditionError> {
    match name {
        "iteration" => Ok(context.iteration.to_string()),
        "user_index" => Ok(context.user_index.to_string()),
        "last_sample_ok" | "JMeterThread.last_sample_ok" => Ok(context.last_sample_ok.to_string()),
        name => {
            context.variables.get(name).cloned().ok_or_else(|| {
                ConditionError::new(format!("undefined condition variable {name:?}"))
            })
        }
    }
}

pub(super) fn expand(input: &str, context: &UserContext) -> Result<String, ConditionError> {
    let mut result = String::new();
    let mut rest = input;
    while let Some(start) = rest.find("${") {
        result.push_str(&rest[..start]);
        rest = &rest[start + 2..];
        let end = rest
            .find('}')
            .ok_or_else(|| ConditionError::new("unterminated condition variable"))?;
        let name = &rest[..end];
        if name.is_empty() || name.contains(['{', '$']) {
            return Err(ConditionError::new("invalid condition variable"));
        }
        result.push_str(&variable(name, context)?);
        rest = &rest[end + 1..];
    }
    result.push_str(rest);
    Ok(result)
}
