use crate::{
    ErrorKind, FunctionContext, FunctionError, MAX_BYTES, MAX_DEPTH, Result, bounded, check_arity,
    invoke,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};

/// Parsed expression; compiling never evaluates functions or reads/writes variables.
#[derive(Debug, Clone)]
pub struct Expression {
    parts: Vec<Part>,
}
#[derive(Debug, Clone)]
enum Part {
    Text(String),
    Variable(String),
    Call(String, Vec<Expression>),
}

impl Expression {
    pub fn compile(input: &str) -> Result<Self> {
        if input.len() > MAX_BYTES {
            return Err(FunctionError::new(
                ErrorKind::Limit,
                "expression exceeds 1 MiB",
            ));
        }
        let mut parser = Parser {
            text: input,
            pos: 0,
        };
        let result = parser.expression(0, false)?;
        if parser.pos != input.len() {
            return Err(syntax("unexpected expression delimiter"));
        }
        Ok(result)
    }
    /// Bounded process-local cache; only syntax is cached, never function results.
    pub fn cached(input: &str) -> Result<Arc<Self>> {
        static CACHE: OnceLock<Mutex<HashMap<String, Arc<Expression>>>> = OnceLock::new();
        let mut cache = CACHE
            .get_or_init(Default::default)
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(value) = cache.get(input) {
            return Ok(value.clone());
        }
        let value = Arc::new(Self::compile(input)?);
        if input.len() <= 8192 {
            if cache.len() >= 256 {
                cache.clear();
            }
            cache.insert(input.to_owned(), value.clone());
        }
        Ok(value)
    }
    /// Writes are transactional for the whole expression. Random/clock reads are not rolled back.
    pub fn evaluate(&self, context: &mut FunctionContext<'_>) -> Result<String> {
        self.evaluate_with(context, &HashMap::new())
    }
    /// Built-in values take precedence over user variables without overwriting them.
    pub fn evaluate_with(
        &self,
        context: &mut FunctionContext<'_>,
        builtins: &HashMap<String, String>,
    ) -> Result<String> {
        let mut variables = context.variables.clone();
        let result = self.eval(
            &mut FunctionContext {
                variables: &mut variables,
                environment: context.environment,
                source: context.source,
            },
            builtins,
        )?;
        *context.variables = variables;
        Ok(result)
    }
    fn eval(
        &self,
        context: &mut FunctionContext<'_>,
        builtins: &HashMap<String, String>,
    ) -> Result<String> {
        let mut result = String::new();
        for part in &self.parts {
            let value = match part {
                Part::Text(text) => text.clone(),
                Part::Variable(name) => builtins
                    .get(name)
                    .or_else(|| context.variables.get(name))
                    .cloned()
                    .ok_or_else(|| {
                        FunctionError::new(
                            ErrorKind::UndefinedVariable,
                            format!("undefined variable {name:?}"),
                        )
                    })?,
                Part::Call(name, arguments) => {
                    let arguments = arguments
                        .iter()
                        .map(|v| v.eval(context, builtins))
                        .collect::<Result<Vec<_>>>()?;
                    invoke(name, &arguments, context)?
                }
            };
            if result.len().saturating_add(value.len()) > MAX_BYTES {
                return Err(FunctionError::new(
                    ErrorKind::Limit,
                    "expanded expression exceeds 1 MiB",
                ));
            }
            result.push_str(&value);
        }
        bounded(result)
    }
}
fn syntax(text: &str) -> FunctionError {
    FunctionError::new(ErrorKind::Syntax, text)
}
struct Parser<'a> {
    text: &'a str,
    pos: usize,
}
impl Parser<'_> {
    fn rest(&self) -> &str {
        &self.text[self.pos..]
    }
    fn expression(&mut self, depth: usize, argument: bool) -> Result<Expression> {
        if depth > MAX_DEPTH {
            return Err(FunctionError::new(
                ErrorKind::Limit,
                "expression nesting exceeds 32",
            ));
        }
        let mut parts = Vec::new();
        let mut literal = String::new();
        while self.pos < self.text.len() {
            if self.rest().starts_with("${") {
                if !literal.is_empty() {
                    parts.push(Part::Text(std::mem::take(&mut literal)));
                }
                self.pos += 2;
                let start = self.pos;
                while let Some(c) = self.rest().chars().next() {
                    if c == '}' || c == '(' {
                        break;
                    }
                    if c == '{' || c == '$' {
                        return Err(syntax("invalid variable name"));
                    }
                    self.pos += c.len_utf8();
                }
                let name = self.text[start..self.pos].to_owned();
                if name.is_empty() {
                    return Err(syntax("empty expression"));
                }
                if name.starts_with("__") && self.rest().starts_with('(') {
                    self.pos += 1;
                    let mut args = Vec::new();
                    loop {
                        args.push(self.expression(depth + 1, true)?);
                        match self.rest().chars().next() {
                            Some(',') => self.pos += 1,
                            Some(')') => {
                                self.pos += 1;
                                break;
                            }
                            _ => return Err(syntax("unterminated function arguments")),
                        }
                    }
                    check_arity(&name, args.len())?;
                    parts.push(Part::Call(name, args));
                } else {
                    // Parentheses in ordinary variable names remain literal for compatibility.
                    while self.rest().starts_with('(')
                        || (!self.rest().starts_with('}') && !self.rest().is_empty())
                    {
                        let c = self.rest().chars().next().unwrap();
                        if c == '{' || c == '$' {
                            return Err(syntax("invalid variable name"));
                        }
                        self.pos += c.len_utf8();
                    }
                    parts.push(Part::Variable(self.text[start..self.pos].to_owned()));
                }
                if !self.rest().starts_with('}') {
                    return Err(syntax("unterminated expression"));
                }
                self.pos += 1;
            } else {
                let c = self.rest().chars().next().unwrap();
                if argument && (c == ',' || c == ')') {
                    break;
                }
                if c == '\\'
                    && let Some(next) = self.rest()[1..].chars().next()
                    && matches!(next, ',' | '(' | ')' | '\\' | '$')
                {
                    self.pos += 1 + next.len_utf8();
                    literal.push(next);
                    continue;
                }
                self.pos += c.len_utf8();
                literal.push(c);
            }
        }
        if !literal.is_empty() {
            parts.push(Part::Text(literal));
        }
        Ok(Expression { parts })
    }
}
