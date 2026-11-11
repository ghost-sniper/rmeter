//! HTTP response extractors. Parsing and evaluation happen in the bounded worker.
use crate::JavaScriptConfigError;
use rmeter_core::context::UserContext;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
    time::Duration,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractorKind {
    Jsonpath,
    Xpath,
    Jmespath,
    Regex,
    Xpath2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtractorSettings {
    pub name: String,
    pub variable_name: String,
    pub expression: String,
    #[serde(default = "first_match")]
    pub match_number: i64,
    #[serde(default)]
    pub default_value: Option<String>,
    #[serde(default = "default_template")]
    pub template: String,
    #[serde(default)]
    pub namespaces: BTreeMap<String, String>,
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    #[serde(default = "default_capacity")]
    pub max_variable_bytes: usize,
    #[serde(default = "default_matches")]
    pub max_matches: usize,
}
fn first_match() -> i64 {
    1
}
fn default_template() -> String {
    "$1$".into()
}
fn default_timeout() -> u64 {
    1000
}
fn default_capacity() -> usize {
    1024 * 1024
}
fn default_matches() -> usize {
    1000
}
impl ExtractorSettings {
    pub fn new(
        name: impl Into<String>,
        variable_name: impl Into<String>,
        expression: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            variable_name: variable_name.into(),
            expression: expression.into(),
            match_number: 1,
            default_value: None,
            template: default_template(),
            namespaces: BTreeMap::new(),
            timeout_ms: default_timeout(),
            max_variable_bytes: default_capacity(),
            max_matches: default_matches(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractorConfig {
    pub kind: ExtractorKind,
    pub settings: ExtractorSettings,
}
#[derive(Debug, Clone)]
pub struct Extractor {
    config: Arc<ExtractorConfig>,
}
impl Extractor {
    pub fn new(config: ExtractorConfig) -> Result<Self, JavaScriptConfigError> {
        validate_settings(&config).map_err(JavaScriptConfigError)?;
        crate::javascript::check_extractor(config.clone()).map_err(|(_, e)| {
            JavaScriptConfigError(format!("extractor {:?}: {e}", config.settings.name))
        })?;
        Ok(Self {
            config: Arc::new(config),
        })
    }
    pub fn config(&self) -> &ExtractorConfig {
        &self.config
    }
    pub(crate) async fn run(
        &self,
        user: &mut UserContext,
        body: Option<&str>,
        reason: Option<&str>,
    ) -> Result<(), (&'static str, String)> {
        // Do not retain stale tokens on a no-match, error, timeout or cancellation.
        clear_outputs(&self.config.settings, &mut user.variables);
        let body = body.ok_or_else(|| {
            (
                "ExtractorError",
                reason
                    .unwrap_or("complete response body unavailable")
                    .to_owned(),
            )
        })?;
        crate::javascript::run_extractor(self.config.clone(), user, body).await
    }
}
fn validate_settings(config: &ExtractorConfig) -> Result<(), String> {
    let s = &config.settings;
    if s.name.trim().is_empty()
        || s.variable_name.trim().is_empty()
        || s.expression.trim().is_empty()
    {
        return Err("name, variable_name and expression must not be empty".into());
    }
    if s.variable_name.trim() != s.variable_name || s.variable_name.contains(['{', '}']) {
        return Err("invalid variable_name".into());
    }
    if s.expression.len() > 1024 * 1024 || s.template.len() > 1024 * 1024 {
        return Err("extractor expression/template exceeds 1 MiB".into());
    }
    if s.match_number < -1 {
        return Err("match_number must be -1 (all), 0 (random), or positive".into());
    }
    if s.timeout_ms == 0
        || s.timeout_ms > 60000
        || s.max_variable_bytes == 0
        || s.max_matches == 0
        || s.max_matches > 100000
    {
        return Err("invalid extractor timeout, capacity or match limit".into());
    }
    if s.variable_name.len()
        + s.default_value.as_ref().map_or(0, String::len)
        + s.variable_name.len()
        + 10
        > s.max_variable_bytes
    {
        return Err("extractor default value exceeds variable capacity".into());
    }
    if !matches!(config.kind, ExtractorKind::Xpath | ExtractorKind::Xpath2)
        && !s.namespaces.is_empty()
    {
        return Err("namespaces are only valid for XPath extractors".into());
    }
    for (prefix, uri) in &s.namespaces {
        if uri.is_empty()
            || prefix == "xmlns"
            || (prefix == "xml" && uri != "http://www.w3.org/XML/1998/namespace")
        {
            return Err("invalid namespace binding".into());
        }
    }
    Ok(())
}
fn numeric(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}
fn output_suffix(s: &str) -> bool {
    s == "matchNr"
        || s == "g"
        || numeric(s)
        || s.strip_prefix('g').is_some_and(numeric)
        || s.split_once("_g")
            .is_some_and(|(n, g)| numeric(n) && (g.is_empty() || numeric(g)))
}
fn clear_outputs(settings: &ExtractorSettings, vars: &mut HashMap<String, String>) {
    let prefix = format!("{}_", settings.variable_name);
    vars.retain(|key, _| {
        key != &settings.variable_name && !key.strip_prefix(&prefix).is_some_and(output_suffix)
    });
    vars.insert(format!("{}_matchNr", settings.variable_name), "0".into());
    if let Some(default) = &settings.default_value {
        vars.insert(settings.variable_name.clone(), default.clone());
    }
}
fn text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        v => v.to_string(),
    }
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Validate inside the worker, including XPath2 compilation, without evaluating input.
pub(crate) fn validate(config: &ExtractorConfig) -> Result<(), String> {
    validate_settings(config)?;
    let s = &config.settings;
    match config.kind {
        ExtractorKind::Jsonpath => {
            serde_json_path::JsonPath::parse(&s.expression).map_err(err)?;
        }
        ExtractorKind::Jmespath => {
            jmespath::compile(&s.expression).map_err(err)?;
        }
        ExtractorKind::Regex => {
            let regex = regex::Regex::new(&s.expression).map_err(err)?;
            for token in template_parts(&s.template)? {
                if let TemplatePart::Group(i) = token
                    && i >= regex.captures_len()
                {
                    return Err(format!("template group ${i}$ does not exist"));
                }
            }
        }
        ExtractorKind::Xpath => {
            let mut c = crate::XmlAssertionConfig::equals(&s.name, &s.expression, "");
            c.namespaces = s.namespaces.clone();
            crate::XmlAssertion::new(c).map_err(err)?;
        }
        ExtractorKind::Xpath2 => {
            let mut context = xee_xpath::context::StaticContextBuilder::default();
            context.namespaces(s.namespaces.iter().map(|(p, u)| (p.as_str(), u.as_str())));
            xee_xpath::Queries::new(context)
                .sequence(&s.expression)
                .map_err(err)?;
        }
    }
    Ok(())
}
#[derive(Debug)]
enum TemplatePart {
    Literal(String),
    Group(usize),
}
fn template_parts(template: &str) -> Result<Vec<TemplatePart>, String> {
    let mut parts = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find('$') {
        parts.push(TemplatePart::Literal(rest[..start].into()));
        rest = &rest[start + 1..];
        let end = rest.find('$').ok_or("template groups must use $N$")?;
        let n = rest[..end]
            .parse()
            .map_err(|_| "template groups must use $N$")?;
        parts.push(TemplatePart::Group(n));
        rest = &rest[end + 1..];
    }
    parts.push(TemplatePart::Literal(rest.into()));
    Ok(parts)
}
struct Match {
    value: String,
    groups: Vec<String>,
}
fn matches(config: &ExtractorConfig, body: &str) -> Result<Vec<Match>, String> {
    let s = &config.settings;
    let values: Vec<String> = match config.kind {
        ExtractorKind::Jsonpath => {
            let data: serde_json::Value =
                serde_json::from_str(body.trim_start_matches('\u{feff}')).map_err(err)?;
            serde_json_path::JsonPath::parse(&s.expression)
                .map_err(err)?
                .query(&data)
                .all()
                .into_iter()
                .map(text)
                .collect()
        }
        ExtractorKind::Jmespath => {
            let data =
                jmespath::Variable::from_json(body.trim_start_matches('\u{feff}')).map_err(err)?;
            let result = jmespath::compile(&s.expression)
                .map_err(err)?
                .search(data)
                .map_err(err)?;
            let value = serde_json::to_value(result).map_err(err)?;
            match value {
                serde_json::Value::Null => vec![],
                serde_json::Value::Array(items) => items.iter().map(text).collect(),
                v => vec![text(&v)],
            }
        }
        ExtractorKind::Xpath => {
            let package = crate::assertion::xml::parse_text(body)?;
            let expression = sxd_xpath::Factory::new()
                .build(&s.expression)
                .map_err(err)?
                .ok_or("empty XPath")?;
            let mut context = sxd_xpath::Context::new();
            for (p, u) in &s.namespaces {
                context.set_namespace(p, u);
            }
            match expression
                .evaluate(&context, package.as_document().root())
                .map_err(err)?
            {
                sxd_xpath::Value::Nodeset(nodes) => nodes
                    .document_order()
                    .iter()
                    .map(|n| n.string_value())
                    .collect(),
                v => vec![v.string()],
            }
        }
        ExtractorKind::Xpath2 => {
            use xee_xpath::Query;
            let mut documents = xee_xpath::Documents::new();
            let doc = documents
                .add_string_without_uri(crate::assertion::xml::checked_text(body)?)
                .map_err(err)?;
            let mut context = xee_xpath::context::StaticContextBuilder::default();
            context.namespaces(s.namespaces.iter().map(|(p, u)| (p.as_str(), u.as_str())));
            let queries = xee_xpath::Queries::new(context);
            let query = queries
                .many(&s.expression, |docs, item| {
                    Ok(item.string_value(docs.xot())?)
                })
                .map_err(err)?;
            // Default dynamic context has no environment variables, documents or resource loaders.
            query.execute(&mut documents, doc).map_err(err)?
        }
        ExtractorKind::Regex => {
            let re = regex::Regex::new(&s.expression).map_err(err)?;
            let parts = template_parts(&s.template)?;
            let mut matches = Vec::new();
            for captures in re.captures_iter(body).take(s.max_matches + 1) {
                let groups: Vec<_> = captures
                    .iter()
                    .map(|v| v.map_or("", |m| m.as_str()).to_owned())
                    .collect();
                let value = parts
                    .iter()
                    .map(|part| match part {
                        TemplatePart::Literal(s) => s.as_str(),
                        TemplatePart::Group(n) => groups[*n].as_str(),
                    })
                    .collect();
                matches.push(Match { value, groups });
            }
            return Ok(matches);
        }
    };
    Ok(values
        .into_iter()
        .map(|value| Match {
            value,
            groups: vec![],
        })
        .collect())
}
pub(crate) fn execute(
    config: &ExtractorConfig,
    body: &str,
    mut vars: HashMap<String, String>,
) -> Result<HashMap<String, String>, String> {
    validate(config)?;
    let s = &config.settings;
    let matches = matches(config, body)?;
    if matches.len() > s.max_matches {
        return Err(format!("matches exceed max_matches ({})", s.max_matches));
    }
    clear_outputs(s, &mut vars);
    vars.insert(
        format!("{}_matchNr", s.variable_name),
        matches.len().to_string(),
    );
    let selected: Vec<(String, &Match)> = match s.match_number {
        -1 => matches
            .iter()
            .enumerate()
            .map(|(i, m)| (format!("{}_{}", s.variable_name, i + 1), m))
            .collect(),
        0 if !matches.is_empty() => vec![(
            s.variable_name.clone(),
            &matches[rand::random_range(0..matches.len())],
        )],
        n if n > 0 => usize::try_from(n - 1)
            .ok()
            .and_then(|i| matches.get(i))
            .map(|m| vec![(s.variable_name.clone(), m)])
            .unwrap_or_default(),
        _ => vec![],
    };
    for (key, m) in selected {
        vars.insert(key.clone(), m.value.clone());
        if !m.groups.is_empty() {
            vars.insert(format!("{key}_g"), (m.groups.len() - 1).to_string());
            for (i, group) in m.groups.iter().enumerate() {
                vars.insert(format!("{key}_g{i}"), group.clone());
            }
        }
    }
    if vars.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>() > s.max_variable_bytes {
        return Err("max_variable_bytes exceeded".into());
    }
    Ok(vars)
}
// Worker transport uses this limit for both the expression and all inherited variables.
pub(crate) fn worker_config(config: &ExtractorConfig) -> crate::JavaScriptConfig {
    let mut script = crate::JavaScriptConfig::new(&config.settings.name, "");
    script.timeout = Duration::from_millis(config.settings.timeout_ms);
    script.max_variable_bytes = config.settings.max_variable_bytes;
    script
}
