//! Shared controller definitions and per-user, context-aware traversal.
//!
//! Direct cursor callers must yield on [`CursorStep::Yield`] to keep loops cancellable.

mod condition;
mod generic_controller;
mod if_controller;
mod while_controller;

pub use condition::{CompareOperator, Condition, ConditionError, Operand};
pub use generic_controller::{Controller, GenericController};
pub use if_controller::IfController;
pub use while_controller::{WhileCondition, WhileController};

use std::fmt;

use crate::{
    context::UserContext,
    plan::{LoopCount, Node},
};
use generic_controller::GenericControllerState;

/// Context-aware traversal. Each cursor owns its controller state; definitions
/// and samplers remain borrowed from the immutable, shared plan.
pub struct ExecutionCursor<'a, S> {
    root: &'a [Node<S>],
    stack: Vec<Frame<'a, S>>,
    has_work: bool,
}

/// One traversal step: a borrowed sampler, cooperative yield, or completion.
pub enum CursorStep<'a, S> {
    Sample(&'a S),
    /// Control traversal exhausted its cooperative work budget without a sample.
    Yield,
    Done,
}

/// A named controller's condition-evaluation failure.
#[derive(Debug)]
pub struct ControllerError {
    pub controller_name: String,
    pub source: ConditionError,
}

impl fmt::Display for ControllerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "controller {:?}: {}", self.controller_name, self.source)
    }
}
impl std::error::Error for ControllerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

struct Frame<'a, S> {
    children: &'a [Node<S>],
    state: GenericControllerState,
    kind: ControllerKind<'a, S>,
}

enum ControllerKind<'a, S> {
    Sequence,
    Loop(LoopCount),
    Generic(&'a GenericController<S>),
    If(&'a IfController<S>),
    While(&'a WhileController<S>),
}

impl<S> ControllerKind<'_, S> {
    fn name(&self) -> &str {
        match self {
            Self::Generic(controller) => &controller.generic().name,
            Self::If(controller) => &controller.generic().name,
            Self::While(controller) => &controller.generic().name,
            Self::Sequence => "sequence",
            Self::Loop(_) => "loop",
        }
    }

    fn enter(&self, context: &UserContext) -> Result<bool, ControllerError> {
        self.wrap(match self {
            Self::Generic(controller) => controller.should_enter(context),
            Self::If(controller) => controller.should_enter(context),
            Self::While(controller) => controller.should_enter(context),
            _ => Ok(true),
        })
    }

    fn repeat(&self, iteration: u64, context: &UserContext) -> Result<bool, ControllerError> {
        self.wrap(match self {
            Self::Loop(LoopCount::Infinite) => Ok(true),
            Self::Loop(LoopCount::Finite(count)) => Ok(iteration < count.saturating_sub(1)),
            Self::Generic(controller) => controller.should_repeat(context),
            Self::If(controller) => controller.should_repeat(context),
            Self::While(controller) => controller.should_repeat(context),
            Self::Sequence => Ok(false),
        })
    }

    fn before_sample(&self, context: &UserContext) -> Result<bool, ControllerError> {
        self.wrap(match self {
            Self::If(controller) => controller.before_sample(context),
            _ => Ok(true),
        })
    }

    fn wrap(&self, result: Result<bool, ConditionError>) -> Result<bool, ControllerError> {
        result.map_err(|source| ControllerError {
            controller_name: self.name().into(),
            source,
        })
    }
}

impl<'a, S> Frame<'a, S> {
    fn new(children: &'a [Node<S>], kind: ControllerKind<'a, S>) -> Self {
        Self {
            children,
            kind,
            state: GenericControllerState::initialize(),
        }
    }

    fn update_index(&self, context: &mut UserContext) {
        if let ControllerKind::While(controller) = self.kind {
            context.variables.insert(
                controller.index_variable(),
                self.state.iteration.to_string(),
            );
        }
    }
}

impl<'a, S> ExecutionCursor<'a, S> {
    /// Start a fresh cursor over borrowed root nodes.
    pub fn new(children: &'a [Node<S>]) -> Self {
        let mut cursor = Self {
            root: children,
            stack: vec![],
            has_work: contains_work(children),
        };
        cursor.reset();
        cursor
    }

    /// Restarts traversal from the first root node, including all nested loops.
    pub fn reset(&mut self) {
        self.stack.clear();
        self.stack
            .push(Frame::new(self.root, ControllerKind::Sequence));
    }

    /// False only for statically empty execution trees. Conditions are work even
    /// when they currently skip everything, since later user iterations may differ.
    pub fn has_work(&self) -> bool {
        self.has_work
    }

    fn check_if_gates(&mut self, context: &UserContext) -> Result<bool, ControllerError> {
        for (index, frame) in self.stack.iter().enumerate() {
            if !frame.state.first && !frame.kind.before_sample(context)? {
                // Preserve the parent, whose child pointer already advanced, and
                // discard this IF's entire nested traversal for the current entry.
                self.stack.truncate(index);
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Advance against the latest context. Callers must yield on CursorStep::Yield
    /// so condition-only/infinite loops remain cancellable on a single-thread runtime.
    pub fn step(
        &mut self,
        context: &mut UserContext,
    ) -> Result<CursorStep<'a, S>, ControllerError> {
        self.check_if_gates(context)?;
        for _ in 0..256 {
            let Some(frame) = self.stack.last_mut() else {
                return Ok(CursorStep::Done);
            };
            if frame.state.first {
                frame.update_index(context);
                frame.state.first = false;
                if !frame.kind.enter(context)? {
                    frame.state.done = true;
                }
            }
            if frame.state.done {
                self.stack.pop();
                continue;
            }
            if frame.state.current == frame.children.len() {
                if !frame.children.is_empty()
                    && frame.kind.repeat(frame.state.iteration, context)?
                {
                    frame.state.reinitialize();
                } else {
                    frame.state.done = true;
                }
                continue;
            }

            let node = &frame.children[frame.state.current];
            frame.state.current += 1;
            let child = match node {
                Node::Sample(sample) => {
                    if self.check_if_gates(context)? {
                        return Ok(CursorStep::Sample(sample));
                    }
                    continue;
                }
                Node::Sequence(children) => Frame::new(children, ControllerKind::Sequence),
                Node::Loop {
                    count: LoopCount::Finite(0),
                    ..
                } => continue,
                Node::Loop { count, children } => {
                    if !contains_work(children) {
                        continue;
                    }
                    Frame::new(children, ControllerKind::Loop(*count))
                }
                Node::Controller(controller) => {
                    Frame::new(&controller.children, ControllerKind::Generic(controller))
                }
                Node::If(controller) => {
                    Frame::new(&controller.generic.children, ControllerKind::If(controller))
                }
                Node::While(controller) => Frame::new(
                    &controller.generic.children,
                    ControllerKind::While(controller),
                ),
            };
            self.stack.push(child);
        }
        Ok(CursorStep::Yield)
    }
}

fn contains_work<S>(children: &[Node<S>]) -> bool {
    let mut pending: Vec<_> = children.iter().collect();
    while let Some(node) = pending.pop() {
        match node {
            Node::Sample(_) | Node::If(_) | Node::While(_) => return true,
            Node::Sequence(children) => pending.extend(children),
            Node::Controller(controller) => pending.extend(&controller.children),
            Node::Loop {
                count: LoopCount::Finite(0),
                ..
            } => {}
            Node::Loop { children, .. } => pending.extend(children),
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn next<'a, S>(cursor: &mut ExecutionCursor<'a, S>) -> Option<&'a S> {
        let mut context = UserContext::new("test", 0);
        loop {
            match cursor.step(&mut context).unwrap() {
                CursorStep::Sample(sample) => return Some(sample),
                CursorStep::Yield => continue,
                CursorStep::Done => return None,
            }
        }
    }

    fn collect<S: Copy>(cursor: &mut ExecutionCursor<'_, S>) -> Vec<S> {
        let mut samples = Vec::new();
        while let Some(sample) = next(cursor) {
            samples.push(*sample);
        }
        samples
    }

    #[test]
    fn traverses_nested_sequences_and_loops_in_order() {
        let nodes = vec![
            Node::Sample(1),
            Node::Loop {
                count: LoopCount::Finite(2),
                children: vec![
                    Node::Sequence(vec![Node::Sample(2), Node::Sample(3)]),
                    Node::Loop {
                        count: LoopCount::Finite(2),
                        children: vec![Node::Sample(4)],
                    },
                ],
            },
            Node::Sample(5),
        ];

        let mut cursor = ExecutionCursor::new(&nodes);
        assert_eq!(collect(&mut cursor), vec![1, 2, 3, 4, 4, 2, 3, 4, 4, 5]);
        assert_eq!(next(&mut cursor), None);
        assert_eq!(next(&mut cursor), None);
    }

    #[test]
    fn skips_zero_count_and_sample_free_loops() {
        let nodes = vec![
            Node::Sequence(vec![]),
            Node::Loop {
                count: LoopCount::Finite(0),
                children: vec![Node::Sample(1)],
            },
            Node::Loop {
                count: LoopCount::Finite(u64::MAX),
                children: vec![Node::Sequence(vec![])],
            },
            Node::Loop {
                count: LoopCount::Infinite,
                children: vec![
                    Node::Sequence(vec![]),
                    Node::Loop {
                        count: LoopCount::Infinite,
                        children: vec![],
                    },
                    Node::Loop {
                        count: LoopCount::Finite(0),
                        children: vec![Node::Sample(2)],
                    },
                ],
            },
            Node::Sample(3),
        ];

        assert_eq!(collect(&mut ExecutionCursor::new(&nodes)), vec![3]);
        assert_eq!(next(&mut ExecutionCursor::<i32>::new(&[])), None);
    }

    #[test]
    fn repeats_infinite_loops_without_reaching_following_nodes() {
        let nodes = vec![
            Node::Sample(0),
            Node::Loop {
                count: LoopCount::Infinite,
                children: vec![Node::Sequence(vec![Node::Sample(1), Node::Sample(2)])],
            },
            Node::Sample(3),
        ];

        let mut cursor = ExecutionCursor::new(&nodes);
        assert_eq!(
            (0..7)
                .map(|_| *next(&mut cursor).unwrap())
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 1, 2, 1, 2]
        );
    }

    #[test]
    fn reset_restarts_partial_and_exhausted_traversals() {
        let nodes = vec![Node::Loop {
            count: LoopCount::Finite(2),
            children: vec![Node::Sample(1), Node::Sample(2)],
        }];
        let mut cursor = ExecutionCursor::new(&nodes);

        for _ in 0..3 {
            next(&mut cursor);
        }
        cursor.reset();
        assert_eq!(collect(&mut cursor), vec![1, 2, 1, 2]);
        cursor.reset();
        assert_eq!(collect(&mut cursor), vec![1, 2, 1, 2]);
    }

    #[test]
    fn reset_exits_an_infinite_loop_and_returns_borrowed_samplers() {
        // This sampler deliberately does not implement Clone or Copy.
        struct Sample;
        let nodes = vec![
            Node::Sample(Sample),
            Node::Loop {
                count: LoopCount::Infinite,
                children: vec![Node::Sample(Sample)],
            },
        ];
        let Node::Sample(first) = &nodes[0] else {
            unreachable!();
        };
        let mut cursor = ExecutionCursor::new(&nodes);

        assert!(std::ptr::eq(next(&mut cursor).unwrap(), first));
        next(&mut cursor);
        next(&mut cursor);
        cursor.reset();
        assert!(std::ptr::eq(next(&mut cursor).unwrap(), first));
    }
}
