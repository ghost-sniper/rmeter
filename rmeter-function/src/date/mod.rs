//! JDK 21 pattern-based ISO date parsing and formatting, implemented in Rust.
//! Bundled locale and zone data is generated offline by `tools/ExportDateData.java`.
mod civil;
mod data;
mod format;
mod parse;
mod pattern;
use crate::{Environment, FunctionError, Result, Source};
use civil::{Civil, month_days, week_start};
use data::LocaleData;
use std::collections::BTreeMap;

#[derive(Clone, Default, Debug)]
pub(crate) struct Fields {
    pub numbers: BTreeMap<char, i64>,
    pub zone: Option<String>,
    pub offset: Option<i32>,
    pub period: Option<Vec<(i64, i64)>>,
}
impl Fields {
    fn put(&mut self, c: char, n: i64) -> Result<()> {
        if self.numbers.insert(c, n).is_some_and(|old| old != n) {
            return Err(FunctionError::date("conflicting date fields"));
        }
        Ok(())
    }
    fn get(&self, c: char) -> Option<i64> {
        self.numbers.get(&c).copied()
    }
}
#[derive(Debug, Clone)]
pub(crate) struct Value {
    fields: Fields,
    date: Option<Civil>,
    time: Option<(i64, i64, i64, i64)>,
    zone: String,
    offset: Option<i32>,
    dst: bool,
}
fn checked(value: i64, min: i64, max: i64) -> Result<i64> {
    if (min..=max).contains(&value) {
        Ok(value)
    } else {
        Err(FunctionError::date("date field out of range"))
    }
}

fn resolve(
    mut f: Fields,
    locale: &LocaleData,
    default_zone: &str,
    defaults: Option<i64>,
) -> Result<Value> {
    if let Some(year) = defaults {
        if f.get('y').is_none() {
            f.put('y', year)?;
        }
        if f.get('M').is_none() {
            f.put('M', 1)?;
        }
        if f.get('d').is_none() {
            f.put('d', 1)?;
        }
    }
    if let Some(y) = f.get('y') {
        checked(y, 1, 1_000_000_000)?;
        let era = f.get('G').unwrap_or(1);
        checked(era, 0, 1)?;
        f.put('u', if era == 0 { 1 - y } else { y })?;
    }
    if let Some(y) = f.get('u')
        && let Some(era) = f.get('G')
        && era != i64::from(y >= 1)
    {
        return Err(FunctionError::date("conflicting era"));
    }
    for (c, min, max) in [
        ('E', 1, 7),
        ('e', 1, 7),
        ('G', 0, 1),
        ('a', 0, 1),
        ('H', 0, 24),
        ('h', 0, 12),
        ('K', 0, 11),
        ('k', 0, 24),
        ('m', 0, 59),
        ('s', 0, 59),
        ('n', 0, 999_999_999),
        ('A', 0, 86_399_999),
        ('N', 0, 86_399_999_999_999),
    ] {
        if let Some(n) = f.get(c) {
            checked(n, min, max)?;
        }
    }
    if let Some(y) = f.get('Y') {
        checked(y, i32::MIN as i64, i32::MAX as i64)?;
    }
    if let Some(e) = f.get('e') {
        f.put('E', (locale.first + e - 2).rem_euclid(7) + 1)?;
    }
    let mut date = None;
    let mut set_date = |candidate: Civil| -> Result<()> {
        if date.is_some_and(|d| d != candidate) {
            return Err(FunctionError::date("conflicting calendar dates"));
        }
        date = Some(candidate);
        Ok(())
    };
    if let Some(g) = f.get('g') {
        set_date(Civil::from_epoch(g.checked_sub(40587).ok_or_else(
            || FunctionError::date("modified Julian day overflow"),
        )?)?)?;
    }
    if let Some(y) = f.get('u') {
        if let (Some(m), Some(d)) = (f.get('M'), f.get('d')) {
            checked(m, 1, 12)?;
            checked(d, 1, 31)?;
            set_date(Civil::new(y, m, d.min(month_days(y, m)))?)?;
        }
        if let Some(d) = f.get('D') {
            checked(d, 1, 366)?;
            let candidate = Civil::from_epoch(Civil::new(y, 1, 1)?.epoch() + d - 1)?;
            if candidate.year != y {
                return Err(FunctionError::date("invalid day of year"));
            }
            set_date(candidate)?;
        }
        if let (Some(m), Some(w), Some(e)) = (f.get('M'), f.get('W'), f.get('E')) {
            checked(m, 1, 12)?;
            checked(w, 0, 6)?;
            set_date(Civil::from_epoch(
                week_start(y, m, locale.first, locale.minimal)
                    + (w - 1) * 7
                    + (e - locale.first).rem_euclid(7),
            )?)?;
        }
    }
    if let (Some(y), Some(w), Some(e)) = (f.get('Y'), f.get('w'), f.get('E')) {
        checked(y, -999_999_999, 999_999_999)?;
        let max = (week_start(y + 1, 1, locale.first, locale.minimal)
            - week_start(y, 1, locale.first, locale.minimal))
            / 7;
        checked(w, 1, max)?;
        set_date(Civil::from_epoch(
            week_start(y, 1, locale.first, locale.minimal)
                + (w - 1) * 7
                + (e - locale.first).rem_euclid(7),
        )?)?;
    }
    if let Some(d) = date {
        for (c, v) in [
            ('E', d.dow()),
            ('Q', (d.month - 1) / 3 + 1),
            ('F', (d.day - 1) / 7 + 1),
            ('Y', civil::week(d, locale.first, locale.minimal).0),
            ('w', civil::week(d, locale.first, locale.minimal).1),
        ] {
            if f.get(c).is_some_and(|n| n != v) {
                return Err(FunctionError::date("conflicting resolved date field"));
            }
        }
    }
    if let Some(n) = f.get('N') {
        f.put('H', n / 3_600_000_000_000)?;
        f.put('m', n / 60_000_000_000 % 60)?;
        f.put('s', n / 1_000_000_000 % 60)?;
        f.put('n', n % 1_000_000_000)?;
    }
    if let Some(n) = f.get('A') {
        f.put('H', n / 3_600_000)?;
        f.put('m', n / 60_000 % 60)?;
        f.put('s', n / 1000 % 60)?;
        if f.get('n').is_some_and(|v| v / 1_000_000 != n % 1000) {
            return Err(FunctionError::date("conflicting fractional seconds"));
        }
        if f.get('n').is_none() {
            f.put('n', (n % 1000) * 1_000_000)?;
        }
    }
    if let Some(h) = f.get('k') {
        f.put('H', h % 24)?;
    }
    if let Some(h) = f.get('h') {
        f.put('K', h % 12)?;
    }
    if let (Some(h), Some(a)) = (f.get('K'), f.get('a')) {
        f.put('H', h + 12 * a)?;
    }
    if let Some(period) = f.period.clone() {
        if let Some(h) = f.get('K')
            && f.get('H').is_none()
        {
            let m = f.get('m').unwrap_or(0);
            let found = [h, h + 12]
                .into_iter()
                .find(|h| {
                    period
                        .iter()
                        .any(|(a, b)| h * 60 + m >= *a && h * 60 + m < *b)
                })
                .ok_or_else(|| FunctionError::date("hour conflicts with day period"))?;
            f.put('H', found)?;
        }
        if f.get('H').is_none() {
            let (a, b) = if period.len() > 1 && period[0].0 == 0 && period.last().unwrap().1 == 1440
            {
                (period.last().unwrap().0, period[0].1 + 1440)
            } else {
                period[0]
            };
            let mid = (a + b) / 2;
            f.put('H', (mid / 60) % 24)?;
            f.put('m', mid % 60)?;
        }
    }
    if f.get('a').is_some() && f.get('H').is_none() && f.get('K').is_none() {
        f.put('H', f.get('a').unwrap() * 12 + 6)?;
    }
    let mut time = None;
    let incomplete = (f.get('m').is_none() && (f.get('s').is_some() || f.get('n').is_some()))
        || (f.get('s').is_none() && f.get('n').is_some());
    if let Some(h) = f.get('H').filter(|_| !incomplete) {
        let m = f.get('m').unwrap_or(0);
        let s = f.get('s').unwrap_or(0);
        let n = f.get('n').unwrap_or(0);
        if h == 24 && (m != 0 || s != 0 || n != 0) {
            return Err(FunctionError::date(
                "24:00 requires zero minute, second and fraction",
            ));
        }
        if h == 24
            && let Some(d) = date
        {
            date = Some(Civil::from_epoch(d.epoch() + 1)?);
        }
        let h = h % 24;
        if f.get('a').is_some_and(|a| a != h / 12) || f.get('K').is_some_and(|k| k != h % 12) {
            return Err(FunctionError::date("conflicting hour fields"));
        }
        if let Some(period) = &f.period
            && !period
                .iter()
                .any(|(a, b)| h * 60 + m >= *a && h * 60 + m < *b)
        {
            return Err(FunctionError::date("time conflicts with day period"));
        }
        time = Some((h, m, s, n));
    }
    let zone = f.zone.clone().unwrap_or_else(|| default_zone.to_owned());
    data::validate_zone(&zone)?;
    let offset = f.offset;
    if f.get('y').is_some() {
        f.numbers.remove(&'y');
        f.numbers.remove(&'G');
    }
    Ok(Value {
        fields: f,
        date,
        time,
        zone,
        offset,
        dst: false,
    })
}
impl Value {
    fn instant(&self) -> Result<Option<i64>> {
        match (self.date, self.time) {
            (Some(d), Some((h, m, s, _))) => {
                let local = d.epoch() * 86400 + h * 3600 + m * 60 + s;
                Ok(Some(if let Some(offset) = self.offset {
                    local - offset as i64
                } else {
                    data::local_instant(&self.zone, local)?
                }))
            }
            _ => Ok(None),
        }
    }
    fn at_instant(seconds: i64, nanos: i64, zone: &str) -> Result<Self> {
        let (offset, dst) = data::offset_at(zone, seconds)?;
        let local = seconds + offset as i64;
        let d = Civil::from_epoch(local.div_euclid(86400))?;
        let t = local.rem_euclid(86400);
        Ok(Self {
            fields: Fields::default(),
            date: Some(d),
            time: Some((t / 3600, t / 60 % 60, t % 60, nanos)),
            zone: zone.to_owned(),
            offset: Some(offset),
            dst,
        })
    }
}
/// Convert Java pattern strings; an empty source pattern means epoch milliseconds.
pub fn date_time_convert(
    text: &str,
    source: &str,
    target: &str,
    environment: &Environment,
) -> Result<String> {
    if text.len() > crate::MAX_BYTES {
        return Err(FunctionError::new(
            crate::ErrorKind::Limit,
            "date input exceeds 1 MiB",
        ));
    }
    let locale = data::locale(&environment.locale)?;
    let target = pattern::compile(target)?;
    let mut value = if source.is_empty() {
        let millis = text
            .parse::<i64>()
            .map_err(|_| FunctionError::date("invalid epoch milliseconds"))?;
        Value::at_instant(
            millis.div_euclid(1000),
            millis.rem_euclid(1000) * 1_000_000,
            &environment.timezone,
        )?
    } else {
        let source = pattern::compile(source)?;
        resolve(
            parse::parse(text, &source, &locale)?,
            &locale,
            &environment.timezone,
            None,
        )?
    };
    if let Some(instant) = value.instant()? {
        if value.zone != environment.timezone {
            value = Value::at_instant(instant, value.time.unwrap().3, &environment.timezone)?;
        } else {
            value.dst = data::offset_at(&value.zone, instant)?.1;
        }
    }
    format::format(&value, &target, &locale)
}
/// Random date sampled uniformly from [start,end), in the supplied format and locale.
pub fn random_date(
    format: &str,
    start: &str,
    end: &str,
    locale: &str,
    environment: &Environment,
    source: &mut dyn Source,
) -> Result<String> {
    if [start, end, locale]
        .iter()
        .any(|s| s.len() > crate::MAX_BYTES)
    {
        return Err(FunctionError::new(
            crate::ErrorKind::Limit,
            "date argument exceeds 1 MiB",
        ));
    }
    let format = if format.trim().is_empty() {
        "yyyy-MM-dd"
    } else {
        format.trim()
    };
    let locale = data::locale(if locale.trim().is_empty() {
        &environment.locale
    } else {
        locale.trim()
    })?;
    let pattern = pattern::compile(format)?;
    let now = source.now_millis();
    let today = Value::at_instant(now.div_euclid(1000), 0, &environment.timezone)?
        .date
        .unwrap();
    let parse_date = |text: &str| -> Result<Civil> {
        resolve(
            parse::parse(text.trim(), &pattern, &locale)?,
            &locale,
            &environment.timezone,
            Some(today.year),
        )?
        .date
        .ok_or_else(|| FunctionError::date("pattern does not resolve a date"))
    };
    let start = if start.trim().is_empty() {
        today
    } else {
        parse_date(start)?
    };
    let end = parse_date(end)?;
    if start.epoch() >= end.epoch() {
        return Err(FunctionError::date("RandomDate end must be after start"));
    }
    let day = crate::random(start.epoch(), end.epoch() - 1, source)?;
    let value = Value {
        fields: Fields::default(),
        date: Some(Civil::from_epoch(day)?),
        time: None,
        zone: environment.timezone.clone(),
        offset: None,
        dst: false,
    };
    format::format(&value, &pattern, &locale)
}
