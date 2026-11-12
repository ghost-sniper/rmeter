use crate::{FunctionError, Result};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};
#[derive(Debug, Clone)]
pub(crate) enum Token {
    Literal(String),
    Field(char, usize),
    Optional(Vec<Token>),
    Pad(usize, Box<Token>),
}
pub(crate) fn numeric(c: char, n: usize) -> bool {
    !matches!(
        c,
        'G' | 'E' | 'a' | 'B' | 'V' | 'v' | 'z' | 'O' | 'X' | 'x' | 'Z'
    ) && !(matches!(c, 'M' | 'L' | 'Q' | 'q' | 'e' | 'c') && n >= 3)
}

pub(crate) fn fixed_width(c: char, n: usize) -> bool {
    numeric(c, n)
        && match c {
            'u' | 'y' | 'Y' => n == 2,
            'A' | 'n' | 'N' | 'g' => false,
            'D' => n == 3,
            'W' | 'e' | 'c' | 'S' => true,
            _ => n > 1,
        }
}
fn valid(c: char, n: usize) -> bool {
    match c {
        'G' | 'E' | 'M' | 'L' | 'Q' | 'q' | 'e' => n <= 5,
        'c' => n <= 5 && n != 2,
        'u' | 'y' | 'Y' | 'A' | 'n' | 'N' | 'g' => n <= 19,
        'D' => n <= 3,
        'd' | 'H' | 'h' | 'K' | 'k' | 'm' | 's' | 'w' => n <= 2,
        'W' | 'F' | 'a' => n == 1,
        'S' => n <= 9,
        'V' => n == 2,
        'v' | 'O' => n == 1 || n == 4,
        'z' => n <= 4,
        'X' | 'x' | 'Z' => n <= 5,
        'B' => n == 1 || n == 4 || n == 5,
        _ => false,
    }
}
pub(crate) fn compile(pattern: &str) -> Result<Arc<Vec<Token>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<Vec<Token>>>>> = OnceLock::new();
    if pattern.len() > 4096 {
        return Err(FunctionError::date("date pattern exceeds 4096 bytes"));
    }
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(p) = cache.get(pattern) {
        return Ok(p.clone());
    }
    let chars: Vec<char> = pattern.chars().collect();
    let mut pos = 0;
    let tokens = Arc::new(parse(&chars, &mut pos, 0)?);
    if cache.len() >= 256 {
        cache.clear();
    }
    cache.insert(pattern.to_owned(), tokens.clone());
    Ok(tokens)
}
fn parse(chars: &[char], pos: &mut usize, depth: usize) -> Result<Vec<Token>> {
    if depth > 32 {
        return Err(FunctionError::date("date optional nesting exceeds 32"));
    }
    let mut out = Vec::new();
    while *pos < chars.len() {
        let c = chars[*pos];
        *pos += 1;
        match c {
            '[' => out.push(Token::Optional(parse(chars, pos, depth + 1)?)),
            ']' => {
                if depth == 0 {
                    return Err(FunctionError::date("unmatched date optional end"));
                }
                return Ok(out);
            }
            '\'' => {
                let mut s = String::new();
                let mut closed = false;
                if chars.get(*pos) == Some(&'\'') {
                    *pos += 1;
                    s.push('\'');
                    closed = true;
                } else {
                    while *pos < chars.len() {
                        let c = chars[*pos];
                        *pos += 1;
                        if c == '\'' {
                            if chars.get(*pos) == Some(&'\'') {
                                *pos += 1;
                                s.push('\'');
                            } else {
                                closed = true;
                                break;
                            }
                        } else {
                            s.push(c);
                        }
                    }
                }
                if !closed {
                    return Err(FunctionError::date("unterminated date literal"));
                }
                out.push(Token::Literal(s));
            }
            '#' | '{' | '}' => return Err(FunctionError::date("reserved date pattern character")),
            c if c.is_ascii_alphabetic() => {
                let mut n = 1;
                while chars.get(*pos) == Some(&c) {
                    *pos += 1;
                    n += 1;
                }
                if c == 'p' {
                    let next = *chars
                        .get(*pos)
                        .ok_or_else(|| FunctionError::date("padding requires a field"))?;
                    *pos += 1;
                    let mut count = 1;
                    while chars.get(*pos) == Some(&next) {
                        *pos += 1;
                        count += 1;
                    }
                    if !valid(next, count) {
                        return Err(FunctionError::date("invalid padded field"));
                    }
                    out.push(Token::Pad(n, Box::new(Token::Field(next, count))));
                } else {
                    if !valid(c, n) {
                        return Err(FunctionError::date(format!(
                            "invalid date pattern field {c} count {n}"
                        )));
                    }
                    out.push(Token::Field(c, n));
                }
            }
            _ => out.push(Token::Literal(c.to_string())),
        }
    }
    Ok(out)
}
