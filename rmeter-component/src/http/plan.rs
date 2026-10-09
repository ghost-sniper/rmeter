use std::collections::HashMap;

use rmeter_core::{
    controller::{Condition, GenericController, IfController, WhileCondition, WhileController},
    plan::{LoopCount, Node},
};

use super::{
    HttpConfigError, HttpRequestConfig, HttpRequestDefaults, HttpSampler, sampler::ClientOptions,
};

/// Component configuration tree, compiled into the core's execution-only tree.
#[derive(Debug, Clone)]
pub enum HttpNode {
    Request(HttpRequestConfig),
    Defaults(HttpRequestDefaults),
    Sequence(Vec<HttpNode>),
    Loop {
        count: LoopCount,
        children: Vec<HttpNode>,
    },
    Controller {
        name: String,
        children: Vec<HttpNode>,
    },
    If {
        name: String,
        condition: Condition,
        evaluate_all: bool,
        children: Vec<HttpNode>,
    },
    While {
        name: String,
        condition: WhileCondition,
        children: Vec<HttpNode>,
    },
}

/// Resolves lexical Defaults scopes without executing configuration nodes.
/// Defaults apply to all requests in their scope, regardless of sibling position.
/// Multiple Defaults at the same level merge in declaration order (later wins).
/// Child overrides do not leak to parents/siblings. Compatible requests share a
/// connection pool; cookies remain disabled. Templates expand at sampling time.
pub fn compile_http_nodes(
    nodes: Vec<HttpNode>,
    defaults: &HttpRequestDefaults,
) -> Result<Vec<Node<HttpSampler>>, HttpConfigError> {
    compile_scope(nodes, defaults, &mut HashMap::new())
}

fn compile_scope(
    nodes: Vec<HttpNode>,
    inherited: &HttpRequestDefaults,
    clients: &mut HashMap<ClientOptions, reqwest::Client>,
) -> Result<Vec<Node<HttpSampler>>, HttpConfigError> {
    let mut defaults = inherited.clone();
    for node in &nodes {
        if let HttpNode::Defaults(inner) = node {
            defaults = defaults.merged_with(inner);
        }
    }
    let mut compiled = Vec::new();
    for node in nodes {
        compiled.push(match node {
            HttpNode::Defaults(_) => continue,
            HttpNode::Request(config) => {
                Node::Sample(HttpSampler::prepare(config, &defaults, clients)?)
            }
            HttpNode::Sequence(children) => {
                Node::Sequence(compile_scope(children, &defaults, clients)?)
            }
            HttpNode::Loop { count, children } => Node::Loop {
                count,
                children: compile_scope(children, &defaults, clients)?,
            },
            HttpNode::Controller { name, children } => Node::Controller(GenericController::new(
                name,
                compile_scope(children, &defaults, clients)?,
            )),
            HttpNode::If {
                name,
                condition,
                evaluate_all,
                children,
            } => {
                let mut controller = IfController::new(
                    name,
                    condition,
                    compile_scope(children, &defaults, clients)?,
                );
                controller.evaluate_all = evaluate_all;
                Node::If(controller)
            }
            HttpNode::While {
                name,
                condition,
                children,
            } => Node::While(WhileController::new(
                name,
                condition,
                compile_scope(children, &defaults, clients)?,
            )),
        });
    }
    Ok(compiled)
}
