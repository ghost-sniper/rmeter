use std::collections::BTreeMap;

use regex::Regex;
use rmeter_core::{
    assertion::{Assertion, AssertionInput, AssertionResult},
    context::UserContext,
};
use sxd_document::{Package, parser};
use sxd_xpath::{Context, Factory, Value};

use super::{AssertionConfigError, MatchSelection, body_text, shorten};

/// XPath string comparison, regex or node-set existence checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum XmlMatcher {
    /// Compare XPath string values, without trimming or numeric coercion.
    Equals(String),
    Regex(String),
    /// Existence matchers require a node-set XPath result.
    Exists,
    NotExists,
}

/// Static XPath 1.0 expression, namespace bindings and matching rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlAssertionConfig {
    pub name: String,
    pub xpath: String,
    pub matcher: XmlMatcher,
    pub selection: MatchSelection,
    pub namespaces: BTreeMap<String, String>,
}
impl XmlAssertionConfig {
    pub fn equals(
        name: impl Into<String>,
        xpath: impl Into<String>,
        expected: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            xpath: xpath.into(),
            matcher: XmlMatcher::Equals(expected.into()),
            selection: MatchSelection::One,
            namespaces: BTreeMap::new(),
        }
    }
}

/// Stores only Send + Sync data. SXD's XPath/DOM are thread-local values and never
/// cross an await point: validate XPath at construction, rebuild it during evaluation.
#[derive(Debug, Clone)]
pub struct XmlAssertion {
    config: XmlAssertionConfig,
    regex: Option<Regex>,
}
impl XmlAssertion {
    pub fn new(config: XmlAssertionConfig) -> Result<Self, AssertionConfigError> {
        let error = |message| AssertionConfigError::new("XML", &config.name, message);
        Factory::new()
            .build(&config.xpath)
            .map_err(|source| error(format!("invalid XPath {:?}: {source}", config.xpath)))?
            .ok_or_else(|| error("XPath must not be empty".into()))?;
        for (prefix, uri) in &config.namespaces {
            if ncname_end(prefix) != Some(prefix.len())
                || prefix == "xmlns"
                || uri.is_empty()
                || (prefix == "xml" && uri != "http://www.w3.org/XML/1998/namespace")
            {
                return Err(error(format!(
                    "invalid namespace binding {prefix:?} = {uri:?}"
                )));
            }
        }
        validate_prefixes(&config.xpath, &config.namespaces).map_err(error)?;
        let regex = match &config.matcher {
            XmlMatcher::Regex(pattern) => Some(
                Regex::new(pattern).map_err(|source| error(format!("invalid regex: {source}")))?,
            ),
            _ => None,
        };
        Ok(Self { config, regex })
    }
    pub fn config(&self) -> &XmlAssertionConfig {
        &self.config
    }
    pub(super) fn failure(&self, message: impl Into<String>) -> AssertionResult {
        AssertionResult::failed(
            &self.config.name,
            format!("XPath {:?}: {}", self.config.xpath, message.into()),
        )
    }
    pub(super) fn evaluate_document(&self, package: &Package) -> AssertionResult {
        let xpath = match Factory::new().build(&self.config.xpath) {
            Ok(Some(xpath)) => xpath,
            _ => return self.failure("cannot compile XPath"),
        };
        let mut context = Context::new();
        context.set_namespace("xml", "http://www.w3.org/XML/1998/namespace");
        for (prefix, uri) in &self.config.namespaces {
            context.set_namespace(prefix, uri);
        }
        let value = match xpath.evaluate(&context, package.as_document().root()) {
            Ok(value) => value,
            Err(error) => return self.failure(format!("evaluation failed: {error}")),
        };
        let values = match value {
            Value::Nodeset(nodes) => nodes
                .document_order()
                .into_iter()
                .map(|node| node.string_value())
                .collect::<Vec<_>>(),
            scalar => {
                if matches!(
                    self.config.matcher,
                    XmlMatcher::Exists | XmlMatcher::NotExists
                ) {
                    return self.failure("existence checks require a node-set result");
                }
                vec![scalar.string()]
            }
        };
        let matched = match &self.config.matcher {
            XmlMatcher::Exists => !values.is_empty(),
            XmlMatcher::NotExists => values.is_empty(),
            matcher => {
                if values.is_empty() {
                    return self.failure("no matching node");
                }
                if self.config.selection == MatchSelection::One && values.len() != 1 {
                    return self
                        .failure(format!("expected exactly one node, found {}", values.len()));
                }
                let matches = |value: &String| match matcher {
                    XmlMatcher::Equals(expected) => value == expected,
                    XmlMatcher::Regex(_) => self.regex.as_ref().unwrap().is_match(value),
                    _ => unreachable!(),
                };
                match self.config.selection {
                    MatchSelection::One | MatchSelection::All => values.iter().all(matches),
                    MatchSelection::Any => values.iter().any(matches),
                }
            }
        };
        if matched {
            return AssertionResult::passed(&self.config.name);
        }
        let expected = match &self.config.matcher {
            XmlMatcher::Equals(value) => format!("equals {:?}", shorten(value)),
            XmlMatcher::Regex(pattern) => format!("string matching regex {:?}", shorten(pattern)),
            XmlMatcher::Exists => "an existing node".into(),
            XmlMatcher::NotExists => "no matching node".into(),
        };
        self.failure(format!(
            "expected {expected} ({:?}); found {} value(s): {:?}",
            self.config.selection,
            values.len(),
            values
                .iter()
                .take(3)
                .map(|v| shorten(v))
                .collect::<Vec<_>>()
        ))
    }
}

// XPath 1.0 strings have no escape syntax. Skip literals and axis separators
// when checking QName prefixes; SXD otherwise panics on an unbound name test.
fn validate_prefixes(xpath: &str, bindings: &BTreeMap<String, String>) -> Result<(), String> {
    let mut rest = xpath;
    while let Some(ch) = rest.chars().next() {
        if ch == '\'' || ch == '"' {
            rest = &rest[1..];
            let Some(end) = rest.find(ch) else {
                break;
            };
            rest = &rest[end + 1..];
        } else if let Some(end) = ncname_end(rest) {
            let name = &rest[..end];
            rest = &rest[end..];
            if let Some(after_axis) = rest.strip_prefix("::") {
                rest = after_axis;
            } else if let Some(after_colon) = rest.strip_prefix(':') {
                if name != "xml" && !bindings.contains_key(name) {
                    return Err(format!("undefined XPath namespace prefix {name:?}"));
                }
                rest = after_colon;
            }
        } else {
            rest = &rest[ch.len_utf8()..];
        }
    }
    Ok(())
}

// XML 1.0 NCName characters (Name without ':'). Names may contain Unicode,
// hyphens and combining marks; leading '-' is an XPath arithmetic operator.
fn ncname_end(text: &str) -> Option<usize> {
    fn start(ch: char) -> bool {
        matches!(ch, '_' | 'A'..='Z' | 'a'..='z' | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}'
            | '\u{F8}'..='\u{2FF}' | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}'
            | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}'
            | '\u{3001}'..='\u{D7FF}' | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}'
            | '\u{10000}'..='\u{EFFFF}')
    }
    let mut chars = text.char_indices();
    if !start(chars.next()?.1) {
        return None;
    }
    Some(chars.find(|(_, ch)| !start(*ch) && !matches!(ch, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}'))
        .map_or(text.len(), |(index, _)| index))
}
impl Assertion for XmlAssertion {
    fn evaluate(&self, input: &AssertionInput<'_>, _context: &UserContext) -> AssertionResult {
        match parse_body(input) {
            Ok(package) => self.evaluate_document(&package),
            Err(error) => self.failure(error),
        }
    }
}

pub(super) fn parse_body(input: &AssertionInput<'_>) -> Result<Package, String> {
    parse_text(body_text(input)?)
}
pub(crate) fn checked_text(text: &str) -> Result<&str, String> {
    let text = text.trim_start_matches('\u{feff}');
    // Disallow DTDs without mistaking literal markers in comments/CDATA for a DTD.
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        rest = &rest[start..];
        let terminator = if rest.starts_with("<!--") {
            Some("-->")
        } else if rest.starts_with("<![CDATA[") {
            Some("]]>")
        } else if rest.starts_with("<?") {
            Some("?>")
        } else {
            None
        };
        if let Some(end) = terminator {
            let Some(index) = rest.find(end) else {
                break;
            };
            rest = &rest[index + end.len()..];
        } else {
            if rest.starts_with("<!DOCTYPE") {
                return Err("XML DOCTYPE is not supported".into());
            }
            rest = &rest[1..];
        }
    }
    Ok(text)
}

pub(crate) fn parse_text(text: &str) -> Result<Package, String> {
    let text = checked_text(text)?;
    let package = parser::parse(text).map_err(|error| format!("invalid XML response: {error}"))?;
    coalesce_text(&package);
    Ok(package)
}

// SXD emits separate text nodes for entities/CDATA. XPath's data model requires
// each adjacent text run to be a single node (including for text() and count()).
fn coalesce_text(package: &Package) {
    use sxd_document::dom::{ChildOfElement, Text};
    fn flush(run: &mut Vec<Text<'_>>) {
        if run.len() > 1 {
            let combined = run.iter().map(|text| text.text()).collect::<String>();
            run[0].set_text(&combined);
            for text in &run[1..] {
                text.remove_from_parent();
            }
        }
        run.clear();
    }
    let mut pending = package
        .as_document()
        .root()
        .children()
        .into_iter()
        .filter_map(|node| node.element())
        .collect::<Vec<_>>();
    while let Some(element) = pending.pop() {
        let mut text_run = Vec::new();
        for child in element.children() {
            match child {
                ChildOfElement::Text(text) => text_run.push(text),
                ChildOfElement::Element(child) => {
                    flush(&mut text_run);
                    pending.push(child);
                }
                _ => flush(&mut text_run),
            }
        }
        flush(&mut text_run);
    }
}
