//! Immutable test plans, thread groups, execution nodes and sample error policies.
//!
//! Plans are shared; mutable state belongs to each virtual user's context.

use std::{collections::HashMap, time::Duration};

use crate::controller::{GenericController, IfController, WhileController};

/// A named plan with initial variables cloned into each virtual user's context.
#[derive(Debug)]
pub struct TestPlan<S> {
    /// Human-readable plan name.
    pub name: String,
    /// Initial string variables copied separately for every virtual user.
    pub variables: HashMap<String, String>,
    /// Groups scheduled concurrently by the engine.
    pub groups: Vec<ThreadGroup<S>>,
}

/// Users executing the same child tree with independent mutable contexts.
#[derive(Debug)]
pub struct ThreadGroup<S> {
    /// Group label included in sample events.
    pub name: String,
    /// Virtual user count; zero disables the group.
    pub users: usize,
    /// Users start at intervals of `ramp_up / users`, starting immediately.
    pub ramp_up: Duration,
    /// Number of complete child-tree traversals per user.
    pub iterations: LoopCount,
    /// Deadline relative to run start, including ramp-up; `None` means no deadline.
    pub max_duration: Option<Duration>,
    /// Action taken after a failed sample, including processor/assertion failures.
    pub on_error: ErrorPolicy,
    /// Root execution nodes, traversed in declaration order.
    pub children: Vec<Node<S>>,
}
/// An execution node; configuration nodes are resolved before execution.
#[derive(Debug)]
pub enum Node<S> {
    /// Execute one sampler.
    Sample(S),
    /// Traverse children once in order.
    Sequence(Vec<Node<S>>),
    /// Repeat children using a cursor-local iteration counter.
    Loop {
        /// Repetition limit for this node.
        count: LoopCount,
        /// Repeated subtree.
        children: Vec<Node<S>>,
    },
    /// Named sequential controller.
    Controller(GenericController<S>),
    /// Conditional subtree.
    If(IfController<S>),
    /// Condition-controlled repetition.
    While(WhileController<S>),
}

/// A finite number of iterations (zero skips execution), or unlimited repetition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopCount {
    /// Execute at most this many iterations.
    Finite(u64),
    /// Repeat until cancelled or otherwise stopped.
    Infinite,
}

/// Action taken after a sample's final outcome is unsuccessful.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorPolicy {
    /// Continue with the next node.
    Continue,
    /// Discard the current traversal and start the next group iteration.
    NextIteration,
    /// Stop only the current virtual user.
    StopUser,
    /// Cancel all virtual users in every group.
    StopTest,
}
