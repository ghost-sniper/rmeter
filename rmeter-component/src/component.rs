use crate::http::sampler::ClientOptions;
use crate::{
    HttpRequestConfig, HttpRequestDefaults, HttpSampler, JavaScriptConfig, JavaScriptSampler,
    jdbc::{JdbcConnection, JdbcConnectionConfig, JdbcSampler, JdbcSamplerConfig},
};
use rmeter_core::{
    context::UserContext,
    controller::{Condition, GenericController, IfController, WhileCondition, WhileController},
    plan::{LoopCount, Node},
    sampler::{SampleError, SampleResult, Sampler},
};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    sync::Arc,
};

#[derive(Debug, Clone)]
pub enum ComponentSampler {
    JavaScript(Box<JavaScriptSampler>),
    Http(Box<HttpSampler>),
    Jdbc(Box<JdbcSampler>),
}
impl ComponentSampler {
    pub fn as_javascript(&self) -> Option<&JavaScriptSampler> {
        match self {
            Self::JavaScript(value) => Some(value),
            _ => None,
        }
    }
    pub fn as_http(&self) -> Option<&HttpSampler> {
        match self {
            Self::Http(value) => Some(value),
            _ => None,
        }
    }
    pub fn as_jdbc(&self) -> Option<&JdbcSampler> {
        match self {
            Self::Jdbc(value) => Some(value),
            _ => None,
        }
    }
}
impl Sampler for ComponentSampler {
    async fn sample(&self, context: &mut UserContext) -> Result<SampleResult, SampleError> {
        match self {
            Self::JavaScript(value) => value.sample(context).await,
            Self::Http(value) => value.sample(context).await,
            Self::Jdbc(value) => value.sample(context).await,
        }
    }
}
#[derive(Debug, Clone)]
pub enum ComponentNode {
    JavaScript(JavaScriptConfig),
    HttpRequest(HttpRequestConfig),
    HttpDefaults(HttpRequestDefaults),
    JdbcConnection(JdbcConnectionConfig),
    JdbcRequest(JdbcSamplerConfig),
    Sequence(Vec<Self>),
    Loop {
        count: LoopCount,
        children: Vec<Self>,
    },
    Controller {
        name: String,
        children: Vec<Self>,
    },
    If {
        name: String,
        condition: Condition,
        evaluate_all: bool,
        children: Vec<Self>,
    },
    While {
        name: String,
        condition: WhileCondition,
        children: Vec<Self>,
    },
}
#[derive(Debug)]
pub struct ComponentConfigError(pub String);
impl fmt::Display for ComponentConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ComponentConfigError {}

/// Compiles HTTP and JDBC nodes together, sharing global pools across groups.
#[derive(Default)]
pub struct ComponentCompiler {
    clients: HashMap<ClientOptions, reqwest::Client>,
    connections: HashMap<String, Arc<JdbcConnection>>,
}
impl ComponentCompiler {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn register_jdbc(
        &mut self,
        config: JdbcConnectionConfig,
    ) -> Result<(), ComponentConfigError> {
        if self.connections.contains_key(&config.variable_name) {
            return Err(ComponentConfigError(format!(
                "duplicate JDBC connection {:?}",
                config.variable_name
            )));
        }
        let connection =
            Arc::new(JdbcConnection::new(config).map_err(|e| ComponentConfigError(e.to_string()))?);
        self.connections
            .insert(connection.config().variable_name.clone(), connection);
        Ok(())
    }
    pub fn compile(
        &mut self,
        nodes: Vec<ComponentNode>,
        defaults: &HttpRequestDefaults,
    ) -> Result<Vec<Node<ComponentSampler>>, ComponentConfigError> {
        self.scope(nodes, defaults, &self.connections.clone())
    }
    fn scope(
        &mut self,
        nodes: Vec<ComponentNode>,
        inherited: &HttpRequestDefaults,
        inherited_jdbc: &HashMap<String, Arc<JdbcConnection>>,
    ) -> Result<Vec<Node<ComponentSampler>>, ComponentConfigError> {
        let mut defaults = inherited.clone();
        let mut connections = inherited_jdbc.clone();
        let mut declared = HashSet::new();
        for node in &nodes {
            match node {
                ComponentNode::HttpDefaults(config) => defaults = defaults.merged_with(config),
                ComponentNode::JdbcConnection(config) => {
                    if !declared.insert(config.variable_name.clone()) {
                        return Err(ComponentConfigError(format!(
                            "duplicate JDBC connection {:?} in one scope",
                            config.variable_name
                        )));
                    }
                    let connection = Arc::new(
                        JdbcConnection::new(config.clone())
                            .map_err(|e| ComponentConfigError(e.to_string()))?,
                    );
                    connections.insert(config.variable_name.clone(), connection);
                }
                _ => {}
            }
        }
        let mut compiled = Vec::new();
        for node in nodes {
            compiled.push(match node {
                ComponentNode::JavaScript(config) => {
                    Node::Sample(ComponentSampler::JavaScript(Box::new(
                        JavaScriptSampler::new(config)
                            .map_err(|e| ComponentConfigError(e.to_string()))?,
                    )))
                }
                ComponentNode::HttpDefaults(_) | ComponentNode::JdbcConnection(_) => continue,
                ComponentNode::HttpRequest(config) => {
                    Node::Sample(ComponentSampler::Http(Box::new(
                        HttpSampler::prepare(config, &defaults, &mut self.clients)
                            .map_err(|e| ComponentConfigError(e.to_string()))?,
                    )))
                }
                ComponentNode::JdbcRequest(config) => {
                    let connection = connections.get(&config.connection).ok_or_else(|| {
                        ComponentConfigError(format!(
                            "JDBC request {:?}: connection {:?} was not configured",
                            config.label, config.connection
                        ))
                    })?;
                    Node::Sample(ComponentSampler::Jdbc(Box::new(
                        JdbcSampler::new(config, connection.clone())
                            .map_err(|e| ComponentConfigError(e.to_string()))?,
                    )))
                }
                ComponentNode::Sequence(children) => {
                    Node::Sequence(self.scope(children, &defaults, &connections)?)
                }
                ComponentNode::Loop { count, children } => Node::Loop {
                    count,
                    children: self.scope(children, &defaults, &connections)?,
                },
                ComponentNode::Controller { name, children } => Node::Controller(
                    GenericController::new(name, self.scope(children, &defaults, &connections)?),
                ),
                ComponentNode::If {
                    name,
                    condition,
                    evaluate_all,
                    children,
                } => {
                    let mut controller = IfController::new(
                        name,
                        condition,
                        self.scope(children, &defaults, &connections)?,
                    );
                    controller.evaluate_all = evaluate_all;
                    Node::If(controller)
                }
                ComponentNode::While {
                    name,
                    condition,
                    children,
                } => Node::While(WhileController::new(
                    name,
                    condition,
                    self.scope(children, &defaults, &connections)?,
                )),
            });
        }
        Ok(compiled)
    }
}
