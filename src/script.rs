use std::{
    collections::{BTreeMap, HashMap},
    fmt,
    path::{Path, PathBuf},
    time::Duration,
};

use rmeter_component::{
    AssertionConfig as ComponentAssertionConfig, ComponentCompiler, ComponentNode,
    ComponentSampler, HttpBody, HttpMethod, HttpRequestConfig, HttpRequestDefaults,
    JdbcConnectionConfig, JdbcQueryType, JdbcSamplerConfig, JsonAssertionConfig, JsonMatcher,
    MatchSelection, RegexAssertionConfig, Setting, XmlAssertionConfig, XmlMatcher,
};
use rmeter_component::{JavaScriptConfig, TimerConfig, TimerValue};
use rmeter_core::{
    controller::{CompareOperator, Condition, Operand, WhileCondition},
    engine::EngineError,
    plan::{ErrorPolicy, LoopCount, TestPlan, ThreadGroup},
};
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug)]
pub enum ScriptError {
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    Json(serde_json::Error),
    Invalid(String),
    Engine(EngineError),
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "cannot read script {}: {source}", path.display())
            }
            Self::Json(error) => write!(f, "invalid JSON script: {error}"),
            Self::Invalid(message) => write!(f, "invalid script: {message}"),
            Self::Engine(error) => write!(f, "execution failed: {error}"),
        }
    }
}
impl std::error::Error for ScriptError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json(error) => Some(error),
            Self::Engine(error) => Some(error),
            Self::Invalid(_) => None,
        }
    }
}
impl From<EngineError> for ScriptError {
    fn from(error: EngineError) -> Self {
        Self::Engine(error)
    }
}

/// Accepts UTF-8 JSON (an optional UTF-8 BOM is allowed), rejects unknown fields,
/// and compiles static HTTP/JSONPath/regex configuration before any request runs.
pub fn parse_script(source: &str) -> Result<TestPlan<ComponentSampler>, ScriptError> {
    let mut value: Value = serde_json::from_str(source.strip_prefix('\u{feff}').unwrap_or(source))
        .map_err(ScriptError::Json)?;
    if !take_enabled(&mut value).map_err(ScriptError::Invalid)? {
        let object = value.as_object_mut().unwrap();
        for key in ["defaults", "variables", "jdbc_connections", "timers"] {
            object.remove(key);
        }
        object.insert("groups".into(), Value::Array(Vec::new()));
    }
    let script: Script = serde_json::from_value(value).map_err(ScriptError::Json)?;
    if script.version != 1 {
        return Err(ScriptError::Invalid(format!(
            "unsupported version {}, expected 1",
            script.version
        )));
    }
    require_name(&script.name, "plan")?;
    let defaults = script.defaults.into_config();
    let mut compiler = ComponentCompiler::new();
    for timer in script.timers.into_iter().filter_map(|v| v.0) {
        compiler
            .register_timer(timer)
            .map_err(|e| ScriptError::Invalid(e.to_string()))?;
    }
    for connection in script
        .jdbc_connections
        .into_iter()
        .filter_map(|item| item.0)
    {
        compiler
            .register_jdbc(connection.into_config())
            .map_err(|error| ScriptError::Invalid(error.to_string()))?;
    }
    let groups = script
        .groups
        .into_iter()
        .enumerate()
        .filter_map(|(index, group)| group.0.map(|group| (index, group)))
        .map(|(index, group)| {
            let location = format!("groups[{index}] ({:?})", group.name);
            require_name(&group.name, &location)?;
            let nodes = group
                .children
                .into_iter()
                .filter_map(|item| item.0)
                .map(NodeConfig::into_node)
                .collect::<Result<Vec<_>, _>>()?;
            let children = compiler
                .compile(nodes, &defaults)
                .map_err(|error| ScriptError::Invalid(format!("{location}: {error}")))?;
            Ok(ThreadGroup {
                name: group.name,
                users: group.users,
                ramp_up: Duration::from_millis(group.ramp_up_ms),
                iterations: group.iterations.into_count(),
                max_duration: group.max_duration_ms.map(Duration::from_millis),
                on_error: group.on_error.into_policy(),
                children,
            })
        })
        .collect::<Result<Vec<_>, ScriptError>>()?;
    Ok(TestPlan {
        name: script.name,
        variables: script.variables,
        groups,
    })
}

pub async fn load_script(
    path: impl AsRef<Path>,
) -> Result<TestPlan<ComponentSampler>, ScriptError> {
    let path = path.as_ref();
    let source = tokio::fs::read_to_string(path)
        .await
        .map_err(|source| ScriptError::Io {
            path: path.to_owned(),
            source,
        })?;
    parse_script(&source)
}

fn require_name(name: &str, location: &str) -> Result<(), ScriptError> {
    if name.trim().is_empty() {
        return Err(ScriptError::Invalid(format!(
            "{location}: name must not be empty"
        )));
    }
    Ok(())
}

// Remove disabled components before deserializing their execution configuration.
// This also skips validation, worker compilation and child configuration scopes.
struct Enabled<T>(Option<T>);
fn take_enabled(value: &mut Value) -> Result<bool, String> {
    let object = value.as_object_mut().ok_or("component must be an object")?;
    match object.remove("enabled") {
        None => Ok(true),
        Some(Value::Bool(enabled)) => Ok(enabled),
        _ => Err("enabled must be a boolean".into()),
    }
}
impl<'de, T: serde::de::DeserializeOwned> Deserialize<'de> for Enabled<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut value = Value::deserialize(deserializer)?;
        if !take_enabled(&mut value).map_err(serde::de::Error::custom)? {
            return Ok(Self(None));
        }
        serde_json::from_value(value)
            .map(|item| Self(Some(item)))
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Script {
    version: u32,
    name: String,
    #[serde(default)]
    variables: HashMap<String, String>,
    #[serde(default)]
    defaults: DefaultsConfig,
    #[serde(default)]
    jdbc_connections: Vec<Enabled<ConnectionConfig>>,
    #[serde(default)]
    timers: Vec<Enabled<TimerConfig>>,
    groups: Vec<Enabled<GroupConfig>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupConfig {
    name: String,
    #[serde(default = "one")]
    users: usize,
    #[serde(default)]
    ramp_up_ms: u64,
    #[serde(default)]
    iterations: CountConfig,
    max_duration_ms: Option<u64>,
    #[serde(default)]
    on_error: PolicyConfig,
    children: Vec<Enabled<NodeConfig>>,
}
fn one() -> usize {
    1
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CountConfig {
    Finite(u64),
    Infinite(Infinite),
}
#[derive(Deserialize)]
enum Infinite {
    #[serde(rename = "infinite")]
    Infinite,
}
impl Default for CountConfig {
    fn default() -> Self {
        Self::Finite(1)
    }
}
impl CountConfig {
    fn into_count(self) -> LoopCount {
        match self {
            Self::Finite(n) => LoopCount::Finite(n),
            Self::Infinite(_) => LoopCount::Infinite,
        }
    }
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PolicyConfig {
    #[default]
    Continue,
    NextIteration,
    StopUser,
    StopTest,
}
impl PolicyConfig {
    fn into_policy(self) -> ErrorPolicy {
        match self {
            Self::Continue => ErrorPolicy::Continue,
            Self::NextIteration => ErrorPolicy::NextIteration,
            Self::StopUser => ErrorPolicy::StopUser,
            Self::StopTest => ErrorPolicy::StopTest,
        }
    }
}

// Missing means inherit; explicit null disables a timeout.
#[derive(Default)]
struct TimeoutConfig(Setting<Option<Duration>>);
impl<'de> Deserialize<'de> for TimeoutConfig {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let millis = Option::<u64>::deserialize(deserializer)?;
        Ok(Self(Setting::Value(millis.map(Duration::from_millis))))
    }
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct DefaultsConfig {
    protocol: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    path: Option<String>,
    headers: Option<Vec<(String, String)>>,
    query: Option<Vec<(String, String)>>,
    connect_timeout_ms: TimeoutConfig,
    request_timeout_ms: TimeoutConfig,
    follow_redirects: Option<bool>,
    max_redirects: Option<usize>,
    save_response_body: Option<bool>,
    max_saved_body_bytes: Option<usize>,
    use_system_proxy: Option<bool>,
}
impl DefaultsConfig {
    fn into_config(self) -> HttpRequestDefaults {
        HttpRequestDefaults {
            protocol: self.protocol,
            host: self.host,
            port: self.port,
            path: self.path,
            headers: self.headers,
            query: self.query,
            connect_timeout: self.connect_timeout_ms.0,
            request_timeout: self.request_timeout_ms.0,
            follow_redirects: self.follow_redirects,
            max_redirects: self.max_redirects,
            save_response_body: self.save_response_body,
            max_saved_body_bytes: self.max_saved_body_bytes,
            use_system_proxy: self.use_system_proxy,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JavaScriptNodeConfig {
    name: String,
    script: String,
    timeout_ms: Option<u64>,
    max_variable_bytes: Option<usize>,
    #[serde(default)]
    timers: Vec<Enabled<TimerConfig>>,
    #[serde(default)]
    pre_processors: Vec<Enabled<PostProcessorConfig>>,
    #[serde(default)]
    post_processors: Vec<Enabled<PostProcessorConfig>>,
}
impl JavaScriptNodeConfig {
    fn into_config(self) -> Result<JavaScriptConfig, ScriptError> {
        require_name(&self.name, "javascript")?;
        let mut config = JavaScriptConfig::new(self.name, self.script);
        if let Some(value) = self.timeout_ms {
            config.timeout = Duration::from_millis(value);
        }
        if let Some(value) = self.max_variable_bytes {
            config.max_variable_bytes = value;
        }
        config.timers = self.timers.into_iter().filter_map(|v| v.0).collect();
        config.pre_processors = self
            .pre_processors
            .into_iter()
            .filter_map(|v| v.0)
            .map(PostProcessorConfig::into_config)
            .collect::<Result<_, _>>()?;
        config.post_processors = self
            .post_processors
            .into_iter()
            .filter_map(|v| v.0)
            .map(PostProcessorConfig::into_config)
            .collect::<Result<_, _>>()?;
        Ok(config)
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum PostProcessorConfig {
    Javascript(JavaScriptNodeConfig),
}
impl PostProcessorConfig {
    fn into_config(self) -> Result<JavaScriptConfig, ScriptError> {
        match self {
            Self::Javascript(config) => {
                if !config.pre_processors.is_empty()
                    || !config.post_processors.is_empty()
                    || !config.timers.is_empty()
                {
                    return Err(ScriptError::Invalid(
                        "processors cannot contain processors or timers".into(),
                    ));
                }
                config.into_config()
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum HttpPostProcessorConfig {
    Javascript(JavaScriptNodeConfig),
    JsonpathExtractor(rmeter_component::extractor::ExtractorSettings),
    XpathExtractor(rmeter_component::extractor::ExtractorSettings),
    JmespathExtractor(rmeter_component::extractor::ExtractorSettings),
    RegexExtractor(rmeter_component::extractor::ExtractorSettings),
    Xpath2Extractor(rmeter_component::extractor::ExtractorSettings),
}
impl HttpPostProcessorConfig {
    fn into_config(self) -> Result<rmeter_component::HttpPostProcessorConfig, ScriptError> {
        use rmeter_component::extractor::{ExtractorConfig, ExtractorKind as K};
        let (kind, settings) = match self {
            Self::Javascript(config) => {
                return Ok(PostProcessorConfig::Javascript(config)
                    .into_config()?
                    .into());
            }
            Self::JsonpathExtractor(s) => (K::Jsonpath, s),
            Self::XpathExtractor(s) => (K::Xpath, s),
            Self::JmespathExtractor(s) => (K::Jmespath, s),
            Self::RegexExtractor(s) => (K::Regex, s),
            Self::Xpath2Extractor(s) => (K::Xpath2, s),
        };
        Ok(ExtractorConfig { kind, settings }.into())
    }
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum NodeConfig {
    ConstantTimer {
        name: String,
        delay_ms: TimerValue,
    },
    GaussianRandomTimer {
        name: String,
        deviation_ms: TimerValue,
        offset_ms: TimerValue,
    },
    Javascript(JavaScriptNodeConfig),
    #[serde(alias = "jdbc_connection")]
    JdbcConnectionConfiguration(ConnectionConfig),
    #[serde(alias = "jdbc_sampler")]
    JdbcRequest {
        name: String,
        connection: String,
        sql: String,
        #[serde(default)]
        query_type: QueryTypeConfig,
        #[serde(default)]
        parameters: Vec<Value>,
        query_timeout_ms: Option<u64>,
        max_rows: Option<usize>,
        max_result_bytes: Option<usize>,
        save_response_body: Option<bool>,
        result_variable: Option<String>,
        #[serde(default)]
        variable_names: Vec<String>,
        row_count_variable: Option<String>,
        #[serde(default)]
        assertions: Vec<Enabled<AssertionConfig>>,
        #[serde(default)]
        timers: Vec<Enabled<TimerConfig>>,
        #[serde(default)]
        pre_processors: Vec<Enabled<PostProcessorConfig>>,
        #[serde(default)]
        post_processors: Vec<Enabled<PostProcessorConfig>>,
    },
    HttpRequest {
        name: String,
        url: Option<String>,
        #[serde(default = "get")]
        method: String,
        #[serde(default)]
        settings: DefaultsConfig,
        #[serde(default)]
        body: BodyConfig,
        #[serde(default)]
        assertions: Vec<Enabled<AssertionConfig>>,
        max_assertion_body_bytes: Option<usize>,
        #[serde(default)]
        timers: Vec<Enabled<TimerConfig>>,
        #[serde(default)]
        pre_processors: Vec<Enabled<PostProcessorConfig>>,
        #[serde(default)]
        post_processors: Vec<Enabled<HttpPostProcessorConfig>>,
        max_post_processor_body_bytes: Option<usize>,
    },
    HttpDefaults {
        name: Option<String>,
        settings: DefaultsConfig,
    },
    Sequence {
        children: Vec<Enabled<NodeConfig>>,
    },
    Loop {
        name: Option<String>,
        count: CountConfig,
        children: Vec<Enabled<NodeConfig>>,
    },
    Controller {
        name: String,
        children: Vec<Enabled<NodeConfig>>,
    },
    If {
        name: String,
        condition: ConditionConfig,
        #[serde(default)]
        evaluate_all: bool,
        children: Vec<Enabled<NodeConfig>>,
    },
    While {
        name: String,
        condition: ConditionConfig,
        children: Vec<Enabled<NodeConfig>>,
    },
}
fn get() -> String {
    "GET".into()
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DriverConfig {
    #[default]
    Postgresql,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum QueryTypeConfig {
    #[default]
    Select,
    Update,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectionConfig {
    name: String,
    variable_name: String,
    url: String,
    #[serde(default)]
    driver: DriverConfig,
    username: Option<String>,
    password: Option<String>,
    password_env: Option<String>,
    max_connections: Option<usize>,
    connect_timeout_ms: Option<u64>,
    acquire_timeout_ms: Option<u64>,
}
impl ConnectionConfig {
    fn into_config(self) -> JdbcConnectionConfig {
        let mut config = JdbcConnectionConfig::new(self.variable_name, self.url);
        config.name = self.name;
        match self.driver {
            DriverConfig::Postgresql => {}
        }
        config.username = self.username;
        config.password = self.password;
        config.password_env = self.password_env;
        if let Some(value) = self.max_connections {
            config.max_connections = value;
        }
        if let Some(value) = self.connect_timeout_ms {
            config.connect_timeout = Duration::from_millis(value);
        }
        if let Some(value) = self.acquire_timeout_ms {
            config.acquire_timeout = Duration::from_millis(value);
        }
        config
    }
}
impl NodeConfig {
    fn into_node(self) -> Result<ComponentNode, ScriptError> {
        let children = |nodes: Vec<Enabled<NodeConfig>>| {
            nodes
                .into_iter()
                .filter_map(|item| item.0)
                .map(Self::into_node)
                .collect::<Result<Vec<_>, _>>()
        };
        Ok(match self {
            Self::ConstantTimer { name, delay_ms } => {
                ComponentNode::Timer(TimerConfig::ConstantTimer { name, delay_ms })
            }
            Self::GaussianRandomTimer {
                name,
                deviation_ms,
                offset_ms,
            } => ComponentNode::Timer(TimerConfig::GaussianRandomTimer {
                name,
                deviation_ms,
                offset_ms,
            }),
            Self::Javascript(config) => ComponentNode::JavaScript(config.into_config()?),
            Self::JdbcConnectionConfiguration(config) => {
                ComponentNode::JdbcConnection(config.into_config())
            }
            Self::JdbcRequest {
                name,
                connection,
                sql,
                query_type,
                parameters,
                query_timeout_ms,
                max_rows,
                max_result_bytes,
                save_response_body,
                result_variable,
                variable_names,
                row_count_variable,
                assertions,
                timers,
                pre_processors,
                post_processors,
            } => {
                require_name(&name, "jdbc_request")?;
                let mut request = JdbcSamplerConfig::new(name, connection, sql);
                request.query_type = match query_type {
                    QueryTypeConfig::Select => JdbcQueryType::Select,
                    QueryTypeConfig::Update => JdbcQueryType::Update,
                };
                request.parameters = parameters;
                if let Some(value) = query_timeout_ms {
                    request.query_timeout = Duration::from_millis(value);
                }
                if let Some(value) = max_rows {
                    request.max_rows = value;
                }
                if let Some(value) = max_result_bytes {
                    request.max_result_bytes = value;
                }
                if let Some(value) = save_response_body {
                    request.save_response_body = value;
                }
                request.result_variable = result_variable;
                request.variable_names = variable_names;
                request.row_count_variable = row_count_variable;
                request.timers = timers.into_iter().filter_map(|v| v.0).collect();
                request.pre_processors = pre_processors
                    .into_iter()
                    .filter_map(|v| v.0)
                    .map(PostProcessorConfig::into_config)
                    .collect::<Result<_, _>>()?;
                request.post_processors = post_processors
                    .into_iter()
                    .filter_map(|item| item.0)
                    .map(PostProcessorConfig::into_config)
                    .collect::<Result<_, _>>()?;
                request.assertions = assertions
                    .into_iter()
                    .filter_map(|item| item.0)
                    .map(AssertionConfig::into_config)
                    .collect::<Result<_, _>>()?;
                ComponentNode::JdbcRequest(request)
            }
            Self::HttpRequest {
                name,
                url,
                method,
                settings,
                body,
                assertions,
                max_assertion_body_bytes,
                timers,
                pre_processors,
                post_processors,
                max_post_processor_body_bytes,
            } => {
                require_name(&name, "http_request")?;
                let mut request = HttpRequestConfig::new(name);
                request.url = url;
                request.method = HttpMethod::from_bytes(method.as_bytes()).map_err(|error| {
                    ScriptError::Invalid(format!(
                        "request {:?}: invalid method {method:?}: {error}",
                        request.label
                    ))
                })?;
                request.settings = settings.into_config();
                request.body = body.into_body();
                request.timers = timers.into_iter().filter_map(|v| v.0).collect();
                request.pre_processors = pre_processors
                    .into_iter()
                    .filter_map(|v| v.0)
                    .map(PostProcessorConfig::into_config)
                    .collect::<Result<_, _>>()?;
                request.post_processors = post_processors
                    .into_iter()
                    .filter_map(|item| item.0)
                    .map(HttpPostProcessorConfig::into_config)
                    .collect::<Result<_, _>>()?;
                if let Some(limit) = max_post_processor_body_bytes {
                    request.max_post_processor_body_bytes = limit;
                }
                request.assertions = assertions
                    .into_iter()
                    .filter_map(|item| item.0)
                    .map(AssertionConfig::into_config)
                    .collect::<Result<Vec<_>, _>>()?;
                if let Some(limit) = max_assertion_body_bytes {
                    request.max_assertion_body_bytes = limit;
                }
                ComponentNode::HttpRequest(request)
            }
            Self::HttpDefaults { name, settings } => {
                if let Some(name) = name {
                    require_name(&name, "http_defaults")?;
                }
                ComponentNode::HttpDefaults(settings.into_config())
            }
            Self::Sequence { children: nodes } => ComponentNode::Sequence(children(nodes)?),
            Self::Loop {
                name,
                count,
                children: nodes,
            } => {
                if let Some(name) = name {
                    require_name(&name, "loop")?;
                }
                ComponentNode::Loop {
                    count: count.into_count(),
                    children: children(nodes)?,
                }
            }
            Self::Controller {
                name,
                children: nodes,
            } => {
                require_name(&name, "controller")?;
                ComponentNode::Controller {
                    name,
                    children: children(nodes)?,
                }
            }
            Self::If {
                name,
                condition,
                evaluate_all,
                children: nodes,
            } => {
                require_name(&name, "if")?;
                ComponentNode::If {
                    name,
                    condition: condition.into_condition(),
                    evaluate_all,
                    children: children(nodes)?,
                }
            }
            Self::While {
                name,
                condition,
                children: nodes,
            } => {
                require_name(&name, "while")?;
                let condition = match condition {
                    ConditionConfig::Expression(text) => WhileCondition::Expression(text),
                    condition => WhileCondition::Condition(condition.into_condition()),
                };
                ComponentNode::While {
                    name,
                    condition,
                    children: children(nodes)?,
                }
            }
        })
    }
}

#[derive(Default, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum BodyConfig {
    #[default]
    Empty,
    Text {
        value: String,
    },
    Bytes {
        value: Vec<u8>,
    },
    Form {
        value: Vec<(String, String)>,
    },
    Json {
        value: Value,
    },
}
impl BodyConfig {
    fn into_body(self) -> HttpBody {
        match self {
            Self::Empty => HttpBody::Empty,
            Self::Text { value } => HttpBody::Text(value),
            Self::Bytes { value } => HttpBody::Bytes(value),
            Self::Form { value } => HttpBody::Form(value),
            Self::Json { value } => HttpBody::Json(value),
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum AssertionConfig {
    Typed(TypedAssertionConfig),
    /// Compatibility with version 1 scripts written before explicit component types.
    LegacyJson(JsonConfig),
}
impl AssertionConfig {
    fn into_config(self) -> Result<ComponentAssertionConfig, ScriptError> {
        match self {
            Self::Typed(TypedAssertionConfig::Json(config)) | Self::LegacyJson(config) => {
                config.into_config().map(Into::into)
            }
            Self::Typed(TypedAssertionConfig::Regex(config)) => {
                require_name(&config.name, "regex assertion")?;
                Ok(RegexAssertionConfig {
                    name: config.name,
                    pattern: config.pattern,
                    invert: config.invert,
                }
                .into())
            }
            Self::Typed(TypedAssertionConfig::Xml(config)) => {
                require_name(&config.name, "XML assertion")?;
                Ok(XmlAssertionConfig {
                    name: config.name,
                    xpath: config.xpath,
                    namespaces: config.namespaces,
                    selection: config.selection.into_selection(),
                    matcher: match config.matcher {
                        XmlMatcherConfig::Equals { expected } => XmlMatcher::Equals(expected),
                        XmlMatcherConfig::Regex { pattern } => XmlMatcher::Regex(pattern),
                        XmlMatcherConfig::Exists => XmlMatcher::Exists,
                        XmlMatcherConfig::NotExists => XmlMatcher::NotExists,
                    },
                }
                .into())
            }
        }
    }
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum TypedAssertionConfig {
    Json(JsonConfig),
    Regex(RegexConfig),
    Xml(XmlConfig),
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegexConfig {
    name: String,
    pattern: String,
    #[serde(default)]
    invert: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct XmlConfig {
    name: String,
    xpath: String,
    matcher: XmlMatcherConfig,
    #[serde(default)]
    selection: SelectionConfig,
    #[serde(default)]
    namespaces: BTreeMap<String, String>,
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum XmlMatcherConfig {
    Equals { expected: String },
    Regex { pattern: String },
    Exists,
    NotExists,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JsonConfig {
    name: String,
    path: String,
    matcher: MatcherConfig,
    #[serde(default)]
    selection: SelectionConfig,
}
impl JsonConfig {
    fn into_config(self) -> Result<JsonAssertionConfig, ScriptError> {
        require_name(&self.name, "assertion")?;
        Ok(JsonAssertionConfig {
            name: self.name,
            path: self.path,
            matcher: match self.matcher {
                MatcherConfig::Equals { expected } => JsonMatcher::Equals(expected),
                MatcherConfig::Regex { pattern } => JsonMatcher::Regex(pattern),
                MatcherConfig::Exists => JsonMatcher::Exists,
                MatcherConfig::NotExists => JsonMatcher::NotExists,
            },
            selection: self.selection.into_selection(),
        })
    }
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum MatcherConfig {
    Equals { expected: Value },
    Regex { pattern: String },
    Exists,
    NotExists,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SelectionConfig {
    #[default]
    One,
    Any,
    All,
}
impl SelectionConfig {
    fn into_selection(self) -> MatchSelection {
        match self {
            Self::One => MatchSelection::One,
            Self::Any => MatchSelection::Any,
            Self::All => MatchSelection::All,
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ConditionConfig {
    Bool(bool),
    Expression(String),
    Rule(Box<RuleConfig>),
}
impl ConditionConfig {
    fn into_condition(self) -> Condition {
        match self {
            Self::Bool(value) => Condition::Constant(value),
            Self::Expression(text) => Condition::Expression(text),
            Self::Rule(rule) => rule.into_condition(),
        }
    }
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum RuleConfig {
    Constant {
        value: bool,
    },
    VariableBool {
        name: String,
    },
    LastSampleOk,
    Expression {
        value: String,
    },
    Compare {
        left: OperandConfig,
        operator: OperatorConfig,
        right: OperandConfig,
    },
    Not {
        condition: ConditionConfig,
    },
    All {
        conditions: Vec<ConditionConfig>,
    },
    Any {
        conditions: Vec<ConditionConfig>,
    },
}
impl RuleConfig {
    fn into_condition(self) -> Condition {
        match self {
            Self::Constant { value } => Condition::Constant(value),
            Self::VariableBool { name } => Condition::VariableBool(name),
            Self::LastSampleOk => Condition::LastSampleOk,
            Self::Expression { value } => Condition::Expression(value),
            Self::Compare {
                left,
                operator,
                right,
            } => Condition::Compare {
                left: left.into_operand(),
                operator: operator.into_operator(),
                right: right.into_operand(),
            },
            Self::Not { condition } => Condition::Not(Box::new(condition.into_condition())),
            Self::All { conditions } => Condition::All(
                conditions
                    .into_iter()
                    .map(ConditionConfig::into_condition)
                    .collect(),
            ),
            Self::Any { conditions } => Condition::Any(
                conditions
                    .into_iter()
                    .map(ConditionConfig::into_condition)
                    .collect(),
            ),
        }
    }
}
#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum OperandConfig {
    Literal { value: String },
    Variable { name: String },
    Iteration,
    UserIndex,
    LastSampleOk,
}
impl OperandConfig {
    fn into_operand(self) -> Operand {
        match self {
            Self::Literal { value } => Operand::Literal(value),
            Self::Variable { name } => Operand::Variable(name),
            Self::Iteration => Operand::Iteration,
            Self::UserIndex => Operand::UserIndex,
            Self::LastSampleOk => Operand::LastSampleOk,
        }
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum OperatorConfig {
    Equal,
    NotEqual,
    Less,
    LessOrEqual,
    Greater,
    GreaterOrEqual,
}
impl OperatorConfig {
    fn into_operator(self) -> CompareOperator {
        match self {
            Self::Equal => CompareOperator::Equal,
            Self::NotEqual => CompareOperator::NotEqual,
            Self::Less => CompareOperator::Less,
            Self::LessOrEqual => CompareOperator::LessOrEqual,
            Self::Greater => CompareOperator::Greater,
            Self::GreaterOrEqual => CompareOperator::GreaterOrEqual,
        }
    }
}
