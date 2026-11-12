use super::{
    Value,
    civil::{week, week_start},
    data::LocaleData,
    pattern::Token,
};
use crate::{FunctionError, Result};
pub(crate) fn format(value: &Value, tokens: &[Token], locale: &LocaleData) -> Result<String> {
    let mut out = String::new();
    for token in tokens {
        match token {
            Token::Literal(s) => out.push_str(s),
            Token::Optional(inner) => match format(value, inner, locale) {
                Ok(s) => out.push_str(&s),
                Err(error) if error.kind == crate::ErrorKind::MissingField => {}
                Err(error) => return Err(error),
            },
            Token::Pad(n, inner) => {
                let s = format(value, std::slice::from_ref(inner.as_ref()), locale)?;
                let length = s.encode_utf16().count();
                if length > *n {
                    return Err(FunctionError::date("date field exceeds pad width"));
                }
                out.extend(std::iter::repeat_n(' ', n - length));
                out.push_str(&s);
            }
            Token::Field(c, n) => out.push_str(&field(value, *c, *n, locale)?),
        }
    }
    Ok(out)
}
fn missing() -> FunctionError {
    FunctionError::new(
        crate::ErrorKind::MissingField,
        "target pattern requires an unavailable date field",
    )
}
fn num(v: i64, n: usize, year: bool) -> String {
    if year && n == 2 {
        return format!("{:02}", v.unsigned_abs() % 100);
    }
    let digits = format!("{:0width$}", v.unsigned_abs(), width = n);
    format!(
        "{}{}",
        if v < 0 {
            "-"
        } else if year && n >= 4 && digits.len() > n {
            "+"
        } else {
            ""
        },
        digits
    )
}
fn field(v: &Value, c: char, n: usize, l: &LocaleData) -> Result<String> {
    let date = || v.date.ok_or_else(missing);
    let time = || v.time.ok_or_else(missing);
    let raw = |c| v.fields.get(c).ok_or_else(missing);
    let year = || {
        v.date
            .map(|d| d.year)
            .or_else(|| v.fields.get('u'))
            .ok_or_else(missing)
    };
    let dow = || v.date.map(|d| d.dow()).map(Ok).unwrap_or_else(|| raw('E'));
    let month = || {
        v.date
            .map(|d| d.month)
            .or_else(|| v.fields.get('M'))
            .ok_or_else(missing)
    };
    let text = |c, n, value: i64| -> Result<String> {
        let index = if matches!(c, 'G' | 'a') {
            Some(value)
        } else {
            value.checked_sub(1)
        };
        Ok(index
            .and_then(|i| usize::try_from(i).ok())
            .and_then(|i| l.texts(c, n).get(i))
            .cloned()
            .unwrap_or_else(|| value.to_string()))
    };
    Ok(match c {
        'u' => num(year()?, n, true),
        'y' => {
            let y = date()?.year;
            num(if y > 0 { y } else { 1 - y }, n, true)
        }
        'G' => text(
            c,
            n,
            if let Some(d) = v.date {
                i64::from(d.year > 0)
            } else {
                raw('G')?
            },
        )?,
        'M' | 'L' => {
            if n >= 3 {
                text(c, n, month()?)?
            } else {
                num(month()?, n, false)
            }
        }
        'Q' | 'q' => {
            let q = if let Some(q) = v.fields.get('Q') {
                q
            } else {
                let m = month()?;
                m.wrapping_add(2) / 3
            };
            if n >= 3 {
                text(c, n, q)?
            } else {
                num(q, n, false)
            }
        }
        'd' => num(
            v.date.map(|d| d.day).map(Ok).unwrap_or_else(|| raw('d'))?,
            n,
            false,
        ),
        'D' => num(
            v.date
                .map(|d| d.ordinal())
                .map(Ok)
                .unwrap_or_else(|| raw('D'))?,
            n,
            false,
        ),
        'g' => num(date()?.epoch() + 40587, n, false),
        'E' => text(c, n, dow()?)?,
        'e' | 'c' => {
            let dow = dow()?;
            if n >= 3 {
                text(c, n, dow)?
            } else {
                num((dow - l.first).rem_euclid(7) + 1, n, false)
            }
        }
        'Y' => num(
            v.date
                .map(|d| week(d, l.first, l.minimal).0)
                .map(Ok)
                .unwrap_or_else(|| raw('Y'))?,
            n,
            true,
        ),
        'w' => num(
            v.date
                .map(|d| week(d, l.first, l.minimal).1)
                .map(Ok)
                .unwrap_or_else(|| raw('w'))?,
            n,
            false,
        ),
        'W' => num(
            v.date
                .map(|d| {
                    (d.epoch() - week_start(d.year, d.month, l.first, l.minimal)).div_euclid(7) + 1
                })
                .map(Ok)
                .unwrap_or_else(|| raw('W'))?,
            n,
            false,
        ),
        'F' => num(
            v.date
                .map(|d| (d.day - 1) / 7 + 1)
                .map(Ok)
                .unwrap_or_else(|| raw('F'))?,
            n,
            false,
        ),
        'H' => num(
            v.time.map(|t| t.0).map(Ok).unwrap_or_else(|| raw('H'))?,
            n,
            false,
        ),
        'k' => {
            let h = time()?.0;
            num(if h == 0 { 24 } else { h }, n, false)
        }
        'K' => num(
            v.time
                .map(|t| t.0 % 12)
                .map(Ok)
                .unwrap_or_else(|| raw('K'))?,
            n,
            false,
        ),
        'h' => {
            let h = time()?.0 % 12;
            num(if h == 0 { 12 } else { h }, n, false)
        }
        'm' => num(
            v.time.map(|t| t.1).map(Ok).unwrap_or_else(|| raw('m'))?,
            n,
            false,
        ),
        's' => num(
            v.time.map(|t| t.2).map(Ok).unwrap_or_else(|| raw('s'))?,
            n,
            false,
        ),
        'n' => num(
            v.time.map(|t| t.3).map(Ok).unwrap_or_else(|| raw('n'))?,
            n,
            false,
        ),
        'S' => format!(
            "{:09}",
            v.time.map(|t| t.3).map(Ok).unwrap_or_else(|| raw('n'))?
        )[..n]
            .to_owned(),
        'N' => {
            let (h, m, s, ns) = time()?;
            num((h * 3600 + m * 60 + s) * 1_000_000_000 + ns, n, false)
        }
        'A' => {
            let (h, m, s, ns) = time()?;
            num((h * 3600 + m * 60 + s) * 1000 + ns / 1_000_000, n, false)
        }
        'a' => text(
            c,
            n,
            v.time
                .map(|t| t.0 / 12)
                .map(Ok)
                .unwrap_or_else(|| raw('a'))?,
        )?,
        'B' => {
            let (h, m, _, _) = time()?;
            l.periods(n)
                .iter()
                .find(|(a, b, _)| h * 60 + m >= *a && h * 60 + m < *b)
                .map(|(_, _, name)| name.clone())
                .ok_or_else(missing)?
        }
        'V' => v.zone.clone(),
        'z' | 'v' => {
            if let Some(names) = l.zones.get(&v.zone) {
                let index = if c == 'v' || v.date.is_none() || v.time.is_none() {
                    if n == 4 { 4 } else { 5 }
                } else {
                    usize::from(super::data::display_dst(&v.zone, v.offset, v.dst)) * 2
                        + usize::from(n != 4)
                };
                names[index].clone()
            } else if v.zone.starts_with(['+', '-']) || v.zone == "Z" {
                v.zone.clone()
            } else if let Some(offset) = v.offset {
                offset_text(offset, 'O', 4, &l.gmt)
            } else {
                v.zone.clone()
            }
        }
        'X' | 'x' | 'Z' | 'O' => offset_text(v.offset.ok_or_else(missing)?, c, n, &l.gmt),
        _ => return Err(missing()),
    })
}
fn offset_text(offset: i32, c: char, n: usize, gmt: &str) -> String {
    if offset == 0 {
        if c == 'X' || c == 'Z' && n == 5 {
            return "Z".into();
        }
        if c == 'O' || c == 'Z' && n == 4 {
            return gmt.into();
        }
    }
    let sign = if offset < 0 { '-' } else { '+' };
    let abs = offset.abs();
    let (h, m, s) = (abs / 3600, abs / 60 % 60, abs % 60);
    if c == 'O' || c == 'Z' && n == 4 {
        let mut out = if n == 4 {
            format!("{gmt}{sign}{h:02}:{m:02}")
        } else {
            format!("{gmt}{sign}{h}")
        };
        if n == 1 && (m != 0 || s != 0) {
            out.push_str(&format!(":{m:02}"));
        }
        if s != 0 {
            out.push_str(&format!(":{s:02}"));
        }
        return out;
    }
    let mut out = format!("{sign}{h:02}");
    let colon = (n == 3 || n == 5) && !(c == 'Z' && n <= 3);
    if c == 'Z' || n != 1 || m != 0 {
        if colon {
            out.push(':');
        }
        out.push_str(&format!("{m:02}"));
    }
    if n >= 4 && s != 0 {
        if colon {
            out.push(':');
        }
        out.push_str(&format!("{s:02}"));
    }
    out
}
