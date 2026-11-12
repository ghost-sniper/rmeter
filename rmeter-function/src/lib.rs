//! Native functions shared by expressions and JavaScript hosts.
//! No Java runtime, JavaScript engine, network access or global variable store is required.
#![doc = include_str!("../docs/en.md")]

#[cfg(doc)]
pub mod translations;

pub mod date;
pub use date::{date_time_convert, random_date};
mod expression;
pub use expression::Expression;
use rand::RngExt;
use std::{
    collections::HashMap,
    fmt,
    time::{SystemTime, UNIX_EPOCH},
};

/// Maximum UTF-8 size of an expression, argument or result.
pub const MAX_BYTES: usize = 1024 * 1024;
/// Maximum generated string length, measured in UTF-16 code units as in JMeter.
pub const MAX_RANDOM_LENGTH: usize = 262_144;
/// Maximum nested expression/function depth.
pub const MAX_DEPTH: usize = 32;

/// Stable error category; diagnostic text does not include argument values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Syntax,
    UnknownFunction,
    Argument,
    Date,
    /// A valid target pattern requests a field absent from the parsed value.
    MissingField,
    UndefinedVariable,
    Limit,
}
/// A function or expression failed without committing its variable writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionError {
    pub kind: ErrorKind,
    pub message: String,
}
impl FunctionError {
    pub(crate) fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
    pub(crate) fn argument(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Argument, message)
    }
    pub(crate) fn date(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Date, message)
    }
}
impl fmt::Display for FunctionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for FunctionError {}
pub type Result<T> = std::result::Result<T, FunctionError>;

/// System defaults captured once by the host at run start and serialized to workers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Environment {
    pub timezone: String,
    pub locale: String,
}
impl Default for Environment {
    fn default() -> Self {
        Self {
            timezone: iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".into()),
            locale: sys_locale::get_locale().unwrap_or_else(|| "en-US".into()),
        }
    }
}

/// Injectable clock and random source. Production uses system time and thread-local randomness.
pub trait Source {
    fn now_millis(&mut self) -> i64;
    fn random_u64(&mut self) -> u64;
}
pub struct SystemSource;
impl Source for SystemSource {
    fn now_millis(&mut self) -> i64 {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(d) => d.as_millis().min(i64::MAX as u128) as i64,
            Err(e) => -(e.duration().as_millis().min(i64::MAX as u128) as i64),
        }
    }
    fn random_u64(&mut self) -> u64 {
        rand::rng().random()
    }
}
/// Context contains no process-global mutable state. Hosts own the variable map.
pub struct FunctionContext<'a> {
    pub variables: &'a mut HashMap<String, String>,
    pub environment: &'a Environment,
    pub source: &'a mut dyn Source,
}

fn bounded(value: String) -> Result<String> {
    if value.len() > MAX_BYTES {
        Err(FunctionError::new(
            ErrorKind::Limit,
            "function output exceeds 1 MiB",
        ))
    } else {
        Ok(value)
    }
}

/// Uniform inclusive signed range, including both i64 extremes, without overflow or modulo bias.
pub fn random(min: i64, max: i64, source: &mut dyn Source) -> Result<i64> {
    if min > max {
        return Err(FunctionError::argument("Random minimum exceeds maximum"));
    }
    let width = (max as i128 - min as i128 + 1) as u128;
    let offset = if width == 1u128 << 64 {
        source.random_u64()
    } else {
        let width = width as u64;
        let threshold = width.wrapping_neg() % width;
        // Bound hostile injected sources as well as pathological entropy failures.
        let mut found = None;
        for _ in 0..1024 {
            let n = source.random_u64();
            if n >= threshold {
                found = Some(n % width);
                break;
            }
        }
        found.ok_or_else(|| {
            FunctionError::new(ErrorKind::Limit, "random source rejection limit exceeded")
        })?
    };
    Ok((min as i128 + offset as i128) as i64)
}

/// Generate valid Unicode with Java UTF-16 length semantics; duplicates weight the character set.
pub fn random_string(
    length: usize,
    chars: Option<&str>,
    source: &mut dyn Source,
) -> Result<String> {
    if length > MAX_RANDOM_LENGTH {
        return Err(FunctionError::new(
            ErrorKind::Limit,
            "RandomString length exceeds 262144 UTF-16 units",
        ));
    }
    if chars.is_some_and(|s| s.len() > MAX_BYTES) {
        return Err(FunctionError::new(
            ErrorKind::Limit,
            "character set exceeds 1 MiB",
        ));
    }
    let chars: Vec<char> = chars.unwrap_or("").trim().chars().collect();
    if length % 2 == 1 && !chars.is_empty() && chars.iter().all(|c| c.len_utf16() == 2) {
        return Err(FunctionError::argument(
            "character set cannot produce requested UTF-16 length",
        ));
    }
    let mut out = String::new();
    let mut remaining = length;
    let mut attempts = 0usize;
    while remaining > 0 {
        attempts += 1;
        if attempts > length.saturating_mul(32).saturating_add(1024) {
            return Err(FunctionError::new(
                ErrorKind::Limit,
                "RandomString random source rejection limit exceeded",
            ));
        }
        let c = if chars.is_empty() {
            // Commons Lang's unrestricted UTF-16 generation excludes private high
            // surrogates and pairs the other surrogate code units.
            let unit = random(0, 65535, source)? as u32;
            if (0xdb80..=0xdbff).contains(&unit) {
                continue;
            }
            if (0xd800..=0xdfff).contains(&unit) {
                if remaining < 2 {
                    continue;
                }
                let high = if unit >= 0xdc00 {
                    random(0xd800, 0xdb7f, source)? as u32
                } else {
                    unit
                };
                let low = if unit >= 0xdc00 {
                    unit
                } else {
                    random(0xdc00, 0xdfff, source)? as u32
                };
                char::from_u32(0x10000 + ((high - 0xd800) << 10) + low - 0xdc00).unwrap()
            } else {
                char::from_u32(unit).unwrap()
            }
        } else if remaining == 1 {
            let eligible: Vec<char> = chars
                .iter()
                .copied()
                .filter(|c| c.len_utf16() == 1)
                .collect();
            eligible[random(0, eligible.len() as i64 - 1, source)? as usize]
        } else {
            chars[random(0, chars.len() as i64 - 1, source)? as usize]
        };
        remaining -= c.len_utf16();
        out.push(c);
    }
    bounded(out)
}

/// Locale-independent case conversion; CAPITALIZE changes only the first character.
pub fn change_case(text: &str, mode: &str) -> Result<String> {
    if text.len() > MAX_BYTES || mode.len() > MAX_BYTES {
        return Err(FunctionError::new(
            ErrorKind::Limit,
            "case argument exceeds 1 MiB",
        ));
    }
    bounded(match mode.to_ascii_uppercase().as_str() {
        "" | "UPPER" => text.to_uppercase(),
        "LOWER" => text.to_lowercase(),
        "CAPITALIZE" => {
            use std::{io::Read, sync::OnceLock};
            static TITLECASE: OnceLock<HashMap<u32, String>> = OnceLock::new();
            let map = TITLECASE.get_or_init(|| {
                let mut text = String::new();
                flate2::read::GzDecoder::new(&include_bytes!("../data/titlecase.json.gz")[..])
                    .read_to_string(&mut text)
                    .expect("titlecase data");
                serde_json::from_str(&text).expect("titlecase JSON")
            });
            match text.chars().next() {
                Some(c) => format!(
                    "{}{}",
                    map.get(&(c as u32))
                        .cloned()
                        .unwrap_or_else(|| c.to_string()),
                    &text[c.len_utf8()..]
                ),
                None => String::new(),
            }
        }
        _ => {
            return Err(FunctionError::argument(
                "changeCase mode must be UPPER, LOWER or CAPITALIZE",
            ));
        }
    })
}

pub(crate) fn signature(name: &str) -> Result<(usize, usize, usize)> {
    match name.strip_prefix("__").unwrap_or(name) {
        "RandomString" => Ok((1, 3, 2)),
        "changeCase" => Ok((1, 3, 2)),
        "dateTimeConvert" => Ok((3, 4, 3)),
        "Random" => Ok((2, 3, 2)),
        "RandomDate" => Ok((3, 5, 4)),
        _ => Err(FunctionError::new(
            ErrorKind::UnknownFunction,
            "unknown function",
        )),
    }
}
pub(crate) fn check_arity(name: &str, count: usize) -> Result<usize> {
    let (min, max, target) = signature(name)?;
    if !(min..=max).contains(&count) {
        return Err(FunctionError::argument(format!(
            "{name} expects {min}..={max} arguments"
        )));
    }
    Ok(target)
}
/// Invoke a named function and optionally store its result. Arguments are already evaluated.
pub fn invoke(name: &str, args: &[String], context: &mut FunctionContext<'_>) -> Result<String> {
    let target = check_arity(name, args.len())?;
    if args.iter().any(|v| v.len() > MAX_BYTES) {
        return Err(FunctionError::new(
            ErrorKind::Limit,
            "function argument exceeds 1 MiB",
        ));
    }
    let arg = |i| args.get(i).map(String::as_str).unwrap_or("");
    let integer = |i| {
        arg(i)
            .trim()
            .parse::<i64>()
            .map_err(|_| FunctionError::argument("expected signed 64-bit integer"))
    };
    let result = match name.strip_prefix("__").unwrap_or(name) {
        "Random" => random(integer(0)?, integer(1)?, context.source)?.to_string(),
        "RandomString" => random_string(
            usize::try_from(integer(0)?)
                .map_err(|_| FunctionError::argument("negative string length"))?,
            Some(arg(1)),
            context.source,
        )?,
        "changeCase" => change_case(arg(0), arg(1))?,
        "dateTimeConvert" => date::date_time_convert(arg(0), arg(1), arg(2), context.environment)?,
        "RandomDate" => date::random_date(
            arg(0),
            arg(1),
            arg(2),
            arg(3),
            context.environment,
            context.source,
        )?,
        _ => unreachable!(),
    };
    let result = bounded(result)?;
    let name = arg(target).trim();
    if !name.is_empty() {
        context.variables.insert(name.to_owned(), result.clone());
    }
    Ok(result)
}
