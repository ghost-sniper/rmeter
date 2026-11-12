//! Proleptic Gregorian arithmetic across java.time's full +/-999,999,999 year range.
use crate::{FunctionError, Result};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Civil {
    pub year: i64,
    pub month: i64,
    pub day: i64,
}
pub(crate) fn leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}
pub(crate) fn month_days(y: i64, m: i64) -> i64 {
    match m {
        2 => {
            if leap(y) {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}
impl Civil {
    pub fn new(year: i64, month: i64, day: i64) -> Result<Self> {
        if !(-999_999_999..=999_999_999).contains(&year)
            || !(1..=12).contains(&month)
            || !(1..=month_days(year, month)).contains(&day)
        {
            return Err(FunctionError::date("invalid ISO date"));
        }
        Ok(Self { year, month, day })
    }
    pub fn epoch(self) -> i64 {
        let y = self.year - i64::from(self.month <= 2);
        let era = y.div_euclid(400);
        let yo = y - era * 400;
        let m = self.month + if self.month > 2 { -3 } else { 9 };
        era * 146097 + yo * 365 + yo / 4 - yo / 100 + (153 * m + 2) / 5 + self.day - 1 - 719468
    }
    pub fn from_epoch(epoch: i64) -> Result<Self> {
        let min = Self {
            year: -999_999_999,
            month: 1,
            day: 1,
        }
        .epoch();
        let max = Self {
            year: 999_999_999,
            month: 12,
            day: 31,
        }
        .epoch();
        if !(min..=max).contains(&epoch) {
            return Err(FunctionError::date("date outside Java ISO range"));
        }
        let z = epoch + 719468;
        let era = z.div_euclid(146097);
        let doe = z - era * 146097;
        let yo = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yo + era * 400;
        let doy = doe - (365 * yo + yo / 4 - yo / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = mp + if mp < 10 { 3 } else { -9 };
        Self::new(y + i64::from(m <= 2), m, d)
    }
    pub fn dow(self) -> i64 {
        (self.epoch() + 3).rem_euclid(7) + 1
    }
    pub fn ordinal(self) -> i64 {
        self.epoch()
            - Self {
                year: self.year,
                month: 1,
                day: 1,
            }
            .epoch()
            + 1
    }
}
pub(crate) fn week_start(year: i64, month: i64, first: i64, minimal: i64) -> i64 {
    let d = Civil {
        year,
        month,
        day: 1,
    };
    let before = (d.dow() - first).rem_euclid(7);
    d.epoch() - before + if 7 - before < minimal { 7 } else { 0 }
}
pub(crate) fn week(date: Civil, first: i64, minimal: i64) -> (i64, i64) {
    let mut y = date.year;
    if date.epoch() < week_start(y, 1, first, minimal) {
        y -= 1;
    } else if date.epoch() >= week_start(y + 1, 1, first, minimal) {
        y += 1;
    }
    (y, (date.epoch() - week_start(y, 1, first, minimal)) / 7 + 1)
}
