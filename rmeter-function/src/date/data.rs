use super::civil::{Civil, month_days};
use crate::{FunctionError, Result};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    io::Read,
    sync::{Arc, Mutex, OnceLock},
};

#[derive(Deserialize)]
pub(crate) struct LocaleData {
    pub patterns: BTreeMap<String, Vec<String>>,
    pub first: i64,
    pub minimal: i64,
    pub gmt: String,
    pub zones: BTreeMap<String, Vec<String>>,
    pub zone_parse: BTreeMap<String, String>,
    pub zone_parse_full: BTreeMap<String, String>,
    #[serde(rename = "B")]
    pub periods_short: Vec<(i64, i64, String)>,
    #[serde(rename = "BBBB")]
    pub periods_full: Vec<(i64, i64, String)>,
    #[serde(rename = "BBBBB")]
    pub periods_narrow: Vec<(i64, i64, String)>,
}
impl LocaleData {
    pub fn periods(&self, n: usize) -> &[(i64, i64, String)] {
        match n {
            4 => &self.periods_full,
            5 => &self.periods_narrow,
            _ => &self.periods_short,
        }
    }
    pub fn texts(&self, c: char, n: usize) -> &[String] {
        let (c, n) = match c {
            'G' => ('G', if n < 4 { 1 } else { n }),
            'E' => ('E', n.max(3)),
            'e' => ('E', n),
            'a' => ('a', 1),
            _ => (c, n),
        };
        self.patterns
            .get(&c.to_string().repeat(n))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}
fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> T {
    let mut text = String::new();
    flate2::read::GzDecoder::new(bytes)
        .read_to_string(&mut text)
        .expect("bundled date data gzip");
    serde_json::from_str(&text).expect("bundled date data JSON")
}
pub(crate) fn locale(name: &str) -> Result<Arc<LocaleData>> {
    static INDEX: OnceLock<BTreeMap<String, (usize, usize)>> = OnceLock::new();
    static CACHE: OnceLock<Mutex<BTreeMap<String, Arc<LocaleData>>>> = OnceLock::new();
    let index = INDEX.get_or_init(|| {
        serde_json::from_str(include_str!("../../data/locales.json")).expect("locale index")
    });
    let name = name.replace('_', "-");
    if name.is_empty()
        || name
            .split('-')
            .any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_alphanumeric()))
    {
        return Err(FunctionError::date("invalid locale identifier"));
    }
    // Java locale fallback: unknown regions inherit language; unknown languages use ROOT.
    let key = if index.contains_key(&name) {
        name
    } else {
        index
            .keys()
            .filter(|key| {
                name.strip_prefix(key.as_str())
                    .is_some_and(|tail| tail.starts_with('-'))
            })
            .max_by_key(|key| key.len())
            .cloned()
            .unwrap_or_else(|| "und".to_owned())
    };
    let mut cache = CACHE
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(data) = cache.get(&key) {
        return Ok(data.clone());
    }
    let (start, len) = index[&key];
    let data: Arc<LocaleData> = Arc::new(decode(
        &include_bytes!("../../data/locales.bin")[start..start + len],
    ));
    if cache.len() >= 8 {
        cache.clear();
    }
    cache.insert(key, data.clone());
    Ok(data)
}
type Rule = (i64, i64, i64, i64, bool, String, i32, i32, i32);
#[derive(Deserialize)]
pub(crate) struct ZoneData {
    initial: i32,
    transitions: Vec<(i64, i32, i32, bool)>,
    rules: Vec<Rule>,
    standard: Vec<(i64, i32)>,
    explicit_dst: i32,
}

pub(crate) fn display_dst(zone: &str, offset: Option<i32>, dst: bool) -> bool {
    if let (Some(zone), Some(offset)) = (zones().get(zone), offset)
        && zone.explicit_dst != i32::MIN
    {
        return offset == zone.explicit_dst;
    }
    dst
}
fn zones() -> &'static BTreeMap<String, ZoneData> {
    static ZONES: OnceLock<BTreeMap<String, ZoneData>> = OnceLock::new();
    ZONES.get_or_init(|| decode(include_bytes!("../../data/zones.json.gz")))
}
pub(crate) fn zone_ids() -> impl Iterator<Item = &'static String> {
    zones().keys()
}
pub(crate) fn offset_id(name: &str) -> Option<i32> {
    let s = name
        .strip_prefix("UTC")
        .or_else(|| name.strip_prefix("GMT"))
        .or_else(|| name.strip_prefix("UT"))
        .unwrap_or(name);
    if s.is_empty() || s == "Z" {
        return Some(0);
    }
    let sign = match s.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let digits = s[1..].replace(':', "");
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (h, m, sec) = match digits.len() {
        1 | 2 => (digits.parse::<i32>().ok()?, 0, 0),
        4 => (digits[..2].parse().ok()?, digits[2..].parse().ok()?, 0),
        6 => (
            digits[..2].parse().ok()?,
            digits[2..4].parse().ok()?,
            digits[4..].parse().ok()?,
        ),
        _ => return None,
    };
    if h > 18 || m > 59 || sec > 59 || (h == 18 && m + sec != 0) {
        None
    } else {
        Some(sign * (h * 3600 + m * 60 + sec))
    }
}
pub(crate) fn validate_zone(name: &str) -> Result<()> {
    if offset_id(name).is_some() || zones().contains_key(name) {
        Ok(())
    } else {
        Err(FunctionError::date("unknown timezone"))
    }
}
fn transition(rule: &Rule, year: i64) -> (i64, i32, i32, bool) {
    let (m, indicator, dow, seconds, end, definition, standard, before, after) = rule;
    let day = if *indicator < 0 {
        month_days(year, *m) + 1 + indicator
    } else {
        *indicator
    };
    let date = Civil {
        year,
        month: *m,
        day,
    };
    let mut epoch = date.epoch();
    if *dow != 0 {
        if *indicator < 0 {
            epoch -= (date.dow() - dow).rem_euclid(7);
        } else {
            epoch += (dow - date.dow()).rem_euclid(7);
        }
    }
    if *end {
        epoch += 1;
    }
    let offset = match definition.as_str() {
        "UTC" => 0,
        "STANDARD" => *standard,
        _ => *before,
    };
    (
        epoch * 86400 + seconds - offset as i64,
        *before,
        *after,
        after != standard,
    )
}
pub(crate) fn offset_at(zone: &str, seconds: i64) -> Result<(i32, bool)> {
    if let Some(offset) = offset_id(zone) {
        return Ok((offset, false));
    }
    let z = zones()
        .get(zone)
        .ok_or_else(|| FunctionError::date("unknown timezone"))?;
    if !z.rules.is_empty() && z.transitions.last().is_none_or(|t| seconds > t.0) {
        let y = Civil::from_epoch(seconds.div_euclid(86400))?.year;
        let mut ts = Vec::new();
        for year in y - 1..=y + 1 {
            for r in &z.rules {
                ts.push(transition(r, year));
            }
        }
        ts.sort_by_key(|t| t.0);
        if let Some(t) = ts.iter().rev().find(|t| seconds >= t.0) {
            return Ok((t.2, t.3));
        }
    }
    let i = z.transitions.partition_point(|t| t.0 <= seconds);
    let offset = if i == 0 {
        z.initial
    } else {
        z.transitions[i - 1].2
    };
    let standard = z.standard[z.standard.partition_point(|t| t.0 <= seconds) - 1].1;
    Ok((offset, offset != standard))
}
/// Java atZone semantics: earlier offset in overlap; move forward by the gap.
pub(crate) fn local_instant(zone: &str, local: i64) -> Result<i64> {
    if let Some(offset) = offset_id(zone) {
        return Ok(local - offset as i64);
    }
    let z = zones()
        .get(zone)
        .ok_or_else(|| FunctionError::date("unknown timezone"))?;
    let mut offsets = vec![z.initial];
    for t in &z.transitions {
        offsets.extend([t.1, t.2]);
    }
    for r in &z.rules {
        offsets.extend([r.7, r.8]);
    }
    offsets.sort();
    offsets.dedup();
    let mut matches = Vec::new();
    for offset in offsets {
        let instant = local - offset as i64;
        if offset_at(zone, instant)?.0 == offset {
            matches.push(instant);
        }
    }
    if let Some(instant) = matches.into_iter().min() {
        return Ok(instant);
    }
    let year = Civil::from_epoch(local.div_euclid(86400))?.year;
    let mut ts = z.transitions.clone();
    for y in year - 1..=year + 1 {
        for r in &z.rules {
            ts.push(transition(r, y));
        }
    }
    for t in ts {
        if t.2 > t.1 && local >= t.0 + t.1 as i64 && local < t.0 + t.2 as i64 {
            return Ok(local - t.1 as i64);
        }
    }
    Err(FunctionError::date("cannot resolve local timezone"))
}
