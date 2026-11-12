use super::{
    Fields,
    data::{self, LocaleData},
    pattern::{Token, fixed_width, numeric},
};
use crate::{FunctionError, Result};
pub(crate) fn parse(text: &str, tokens: &[Token], locale: &LocaleData) -> Result<Fields> {
    let mut fields = Fields::default();
    let mut pos = 0;
    read(text, &mut pos, tokens, locale, &mut fields)?;
    if pos != text.len() {
        return Err(FunctionError::date("unparsed date text"));
    }
    Ok(fields)
}
fn fail() -> FunctionError {
    FunctionError::date("date text does not match pattern")
}
fn read(
    text: &str,
    pos: &mut usize,
    tokens: &[Token],
    locale: &LocaleData,
    f: &mut Fields,
) -> Result<()> {
    for (i, token) in tokens.iter().enumerate() {
        match token {
            Token::Literal(s) => {
                if !text[*pos..].starts_with(s) {
                    return Err(fail());
                }
                *pos += s.len();
            }
            Token::Optional(inner) => {
                let old = *pos;
                let snapshot = f.clone();
                if read(text, pos, inner, locale, f).is_err() {
                    *pos = old;
                    *f = snapshot;
                }
            }
            Token::Pad(width, inner) => {
                let end = text[*pos..]
                    .char_indices()
                    .nth(*width)
                    .map(|(i, _)| *pos + i)
                    .unwrap_or(text.len());
                if text[*pos..end].chars().count() != *width {
                    return Err(fail());
                }
                while text[*pos..end].starts_with(' ') {
                    *pos += 1;
                }
                read(
                    &text[..end],
                    pos,
                    std::slice::from_ref(inner.as_ref()),
                    locale,
                    f,
                )?;
                if *pos != end {
                    return Err(fail());
                }
            }
            Token::Field(c, n) => {
                if numeric(*c, *n) {
                    let reserve = tokens[i + 1..]
                        .iter()
                        .take_while(
                            |token| matches!(token, Token::Field(c,n) if fixed_width(*c,*n)),
                        )
                        .map(|token| {
                            if let Token::Field(_, n) = token {
                                *n
                            } else {
                                0
                            }
                        })
                        .sum();
                    let value = number(text, pos, *c, *n, reserve)?;
                    let c = match c {
                        'L' => 'M',
                        'q' => 'Q',
                        'c' => 'e',
                        _ => *c,
                    };
                    f.put(
                        if c == 'S' { 'n' } else { c },
                        if c == 'S' {
                            value * 10i64.pow(9 - *n as u32)
                        } else {
                            value
                        },
                    )?;
                } else if matches!(c, 'X' | 'x' | 'Z' | 'O') {
                    let (offset, len) = parse_offset(&text[*pos..], *c, *n, &locale.gmt)?;
                    if f.offset.is_some_and(|old| old != offset) {
                        return Err(fail());
                    }
                    f.offset = Some(offset);
                    *pos += len;
                } else if matches!(c, 'V' | 'v' | 'z') {
                    let s = &text[*pos..];
                    let mut candidates: Vec<(&str, String)> = Vec::new();
                    for id in data::zone_ids() {
                        if s.starts_with(id) {
                            candidates.push((id, id.clone()));
                        }
                    }
                    if *c != 'V' {
                        for (name, id) in if *n == 4 {
                            &locale.zone_parse_full
                        } else {
                            &locale.zone_parse
                        } {
                            if s.starts_with(name) {
                                candidates.push((name, id.clone()));
                            }
                        }
                    }
                    for prefix in ["UTC", "GMT", "UT", ""] {
                        if let Some(rest) = s.strip_prefix(prefix) {
                            if rest.starts_with('+') || rest.starts_with('-') {
                                for len in [9, 6, 3] {
                                    if rest.len() >= len && rest.is_char_boundary(len) {
                                        let candidate = &rest[..len];
                                        if data::offset_id(candidate).is_some() {
                                            candidates.push((
                                                &s[..prefix.len() + len],
                                                s[..prefix.len() + len].to_owned(),
                                            ));
                                            break;
                                        }
                                    }
                                }
                            } else if !prefix.is_empty() {
                                candidates.push((&s[..prefix.len()], prefix.to_owned()));
                            }
                        }
                    }
                    if s.starts_with('Z') {
                        candidates.push((&s[..1], "Z".into()));
                    }
                    let (name, id) = candidates
                        .into_iter()
                        .max_by_key(|(s, _)| s.len())
                        .ok_or_else(fail)?;
                    *pos += name.len();
                    if f.zone.as_ref().is_some_and(|old| old != &id) {
                        return Err(fail());
                    }
                    f.zone = Some(id);
                } else if *c == 'B' {
                    let ranges = locale.periods(*n);
                    let (_, _, name) = ranges
                        .iter()
                        .filter(|(_, _, s)| text[*pos..].starts_with(s))
                        .max_by_key(|(_, _, s)| s.len())
                        .ok_or_else(fail)?;
                    *pos += name.len();
                    f.period = Some(
                        ranges
                            .iter()
                            .filter(|(_, _, s)| s == name)
                            .map(|(a, b, _)| {
                                (if b - a > 1 && a % 60 == 1 { a - 1 } else { *a }, *b)
                            })
                            .collect(),
                    );
                } else {
                    let values = locale.texts(*c, *n);
                    let (index, name) = values
                        .iter()
                        .enumerate()
                        .filter(|(_, s)| text[*pos..].starts_with(s.as_str()))
                        .max_by_key(|(_, s)| s.len())
                        .ok_or_else(fail)?;
                    *pos += name.len();
                    let field = match c {
                        'L' => 'M',
                        'q' => 'Q',
                        'e' | 'c' => 'E',
                        _ => *c,
                    };
                    let value = index as i64 + if matches!(c, 'G' | 'a') { 0 } else { 1 };
                    f.put(field, value)?;
                }
            }
        }
    }
    Ok(())
}
fn number(text: &str, pos: &mut usize, c: char, n: usize, reserve: usize) -> Result<i64> {
    let start = *pos;
    let mut negative = false;
    let mut plus = false;
    if let Some(sign) = text.as_bytes().get(*pos)
        && (*sign == b'-' || *sign == b'+')
    {
        if matches!(c, 'w' | 'W' | 'e' | 'c' | 'A' | 'n' | 'N')
            || !(n == 1 || matches!(c, 'y' | 'u' | 'Y' | 'g'))
            || (matches!(c, 'y' | 'u' | 'Y') && n == 2)
            || (*sign == b'+' && !matches!(c, 'y' | 'u' | 'Y'))
        {
            return Err(fail());
        }
        negative = *sign == b'-';
        plus = *sign == b'+';
        *pos += 1;
    }
    let available = text[*pos..]
        .bytes()
        .take_while(|b| b.is_ascii_digit())
        .count();
    let (min, max) = match c {
        'D' if n > 1 => (n, 3),
        'w' => (n, 2),
        'W' | 'e' | 'c' => (n, n),
        'S' => (n, n),
        'u' | 'y' | 'Y' if n == 2 => (2, 2),
        'u' | 'y' | 'Y' => (n, 19),
        'A' | 'n' | 'N' | 'g' => (n, 19),
        _ if n == 1 => (1, 19),
        _ => (n, n),
    };
    let width = available.saturating_sub(reserve).min(max);
    if width < min {
        return Err(fail());
    }
    if matches!(c, 'u' | 'y' | 'Y')
        && n >= 4
        && ((plus && width <= n) || (!plus && !negative && width > n))
    {
        return Err(fail());
    }
    if plus && matches!(c, 'u' | 'y' | 'Y') && n < 4 {
        return Err(fail());
    }
    *pos += width;
    let mut value = text[start..*pos].parse::<i64>().map_err(|_| fail())?;
    if negative && value == 0 {
        return Err(fail());
    }
    if matches!(c, 'u' | 'y' | 'Y') && n == 2 {
        value += 2000;
    }
    Ok(value)
}
fn parse_offset(text: &str, c: char, n: usize, gmt: &str) -> Result<(i32, usize)> {
    if (c == 'X' || c == 'Z' && n == 5) && text.starts_with('Z') {
        return Ok((0, 1));
    }
    if c == 'O' || c == 'Z' && n == 4 {
        let rest = text.strip_prefix(gmt).ok_or_else(fail)?;
        if !rest.starts_with(['+', '-']) {
            return Ok((0, gmt.len()));
        }
        let max = rest
            .bytes()
            .take_while(|b| b.is_ascii_digit() || *b == b'+' || *b == b'-' || *b == b':')
            .count()
            .min(9);
        for len in (2..=max).rev() {
            let candidate = &rest[..len];
            let norm = if candidate.as_bytes().get(2) == Some(&b':') {
                format!("{}0{}", &candidate[..1], &candidate[1..])
            } else {
                candidate.to_owned()
            };
            if let Some(offset) = data::offset_id(&norm) {
                if n == 4 && len < 6 {
                    continue;
                }
                return Ok((offset, gmt.len() + len));
            }
        }
        return Err(fail());
    }
    let lengths: &[usize] = match (c, n) {
        ('X' | 'x', 1) => &[5, 3],
        ('X' | 'x', 2) => &[5],
        ('X' | 'x', 3) => &[6],
        ('X' | 'x', 4) => &[7, 5],
        ('X' | 'x', 5) => &[9, 6],
        ('Z', 1..=3) => &[5],
        ('Z', 5) => &[9, 6],
        _ => return Err(fail()),
    };
    for len in lengths {
        if text.len() < *len || !text.is_char_boundary(*len) {
            continue;
        }
        let part = &text[..*len];
        let colon = matches!(n, 3 | 5) && c != 'Z' || c == 'Z' && n == 5;
        if colon && part.as_bytes().get(3) != Some(&b':') {
            continue;
        }
        if !colon && part.contains(':') {
            continue;
        }
        if let Some(offset) = numeric_offset(part) {
            return Ok((offset, *len));
        }
    }
    Err(fail())
}

fn numeric_offset(text: &str) -> Option<i32> {
    let sign = match text.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let digits = text[1..].replace(':', "");
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (h, m, s) = match digits.len() {
        2 => (digits.parse::<i32>().ok()?, 0, 0),
        4 => (digits[..2].parse().ok()?, digits[2..].parse().ok()?, 0),
        6 => (
            digits[..2].parse().ok()?,
            digits[2..4].parse().ok()?,
            digits[4..].parse().ok()?,
        ),
        _ => return None,
    };
    if h > 23 || m > 59 || s > 59 {
        None
    } else {
        Some(sign * (h * 3600 + m * 60 + s))
    }
}
