use std::{collections::HashMap, time::Duration};

use crate::controller::{GenericController, IfController, WhileController};

#[derive(Debug)]
pub struct TestPlan<S> {
    pub name: String,
    pub variables: HashMap<String, String>,
    pub groups: Vec<ThreadGroup<S>>,
}

#[derive(Debug)]
pub struct ThreadGroup<S> {
    pub name: String,
    pub users: usize,
    pub ramp_up: Duration,
    pub iterations: LoopCount,
    pub max_duration: Option<Duration>,
    pub on_error: ErrorPolicy,
    pub children: Vec<Node<S>>,
}
#[derive(Debug)]
pub enum Node<S> {
    Sample(S),
    Sequence(Vec<Node<S>>),
    Loop {
        count: LoopCount,
        children: Vec<Node<S>>,
    },
    Controller(GenericController<S>),
    If(IfController<S>),
    While(WhileController<S>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopCount {
    Finite(u64),
    Infinite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorPolicy {
    Continue,
    NextIteration,
    StopUser,
    StopTest,
}
