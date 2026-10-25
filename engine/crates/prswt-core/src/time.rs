//! Integer time model ([[ADR-0002-integer-time]]).
//!
//! * [`Instant`] — wall-clock minutes since 2000-01-01T00:00 in project-local time.
//! * [`WorkMinutes`] — a signed amount of *working* time.
//!
//! Durations exchanged with humans/agents are strings like `"5d"`, `"-2h"`,
//! `"1.5d"`, `"90m"`; days convert with the calendar's minutes-per-day.

use std::fmt;
use std::ops::{Add, AddAssign, Neg, Sub, SubAssign};
use std::str::FromStr;

use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, Timelike};
use rust_decimal::prelude::ToPrimitive;
use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};

/// Minutes in an hour.
pub const MINUTES_PER_HOUR: i64 = 60;
/// Minutes in a calendar day.
pub const MINUTES_PER_DAY: i64 = 1440;

/// 2000-01-01 is 10,957 days after the Unix epoch.
const EPOCH_DAYS_AFTER_UNIX: i64 = 10_957;

fn epoch() -> NaiveDateTime {
    DateTime::UNIX_EPOCH.naive_utc() + TimeDelta::days(EPOCH_DAYS_AFTER_UNIX)
}

/// A point in project-local wall-clock time, minute resolution.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Instant(pub i64);

impl Instant {
    /// Earliest representable instant (with headroom for arithmetic).
    pub const MIN: Instant = Instant(i64::MIN / 4);
    /// Latest representable instant (with headroom for arithmetic).
    pub const MAX: Instant = Instant(i64::MAX / 4);

    /// Instant from a local date-time (truncated to the minute).
    pub fn from_datetime(dt: NaiveDateTime) -> Self {
        Self((dt - epoch()).num_minutes())
    }

    /// Instant at midnight starting `d`.
    pub fn from_date(d: NaiveDate) -> Self {
        Self::from_datetime(d.and_time(NaiveTime::MIN))
    }

    /// `None` for an invalid calendar date or time of day.
    pub fn from_ymd_hm(y: i32, m: u32, d: u32, hh: u32, mm: u32) -> Option<Self> {
        let date = NaiveDate::from_ymd_opt(y, m, d)?;
        let time = NaiveTime::from_hms_opt(hh, mm, 0)?;
        Some(Self::from_datetime(date.and_time(time)))
    }

    /// `None` when the instant lies outside chrono's representable range.
    pub fn to_datetime(self) -> Option<NaiveDateTime> {
        epoch().checked_add_signed(TimeDelta::try_minutes(self.0)?)
    }

    /// Whole days since the epoch (floor division, correct for negatives).
    pub fn day_index(self) -> i64 {
        self.0.div_euclid(MINUTES_PER_DAY)
    }

    /// Minute within the day, `0..1440`.
    pub fn minute_of_day(self) -> i64 {
        self.0.rem_euclid(MINUTES_PER_DAY)
    }

    /// Calendar date, or `None` outside chrono's range.
    pub fn date(self) -> Option<NaiveDate> {
        self.to_datetime().map(|dt| dt.date())
    }

    /// Start of day `day_index`.
    pub fn from_day_index(day: i64) -> Self {
        Self(day.saturating_mul(MINUTES_PER_DAY))
    }

    /// Instant shifted by `m` wall-clock minutes (saturating).
    pub fn plus_minutes(self, m: i64) -> Self {
        Self(self.0.saturating_add(m))
    }
}

impl fmt::Display for Instant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_datetime() {
            Some(dt) => write!(
                f,
                "{:04}-{:02}-{:02}T{:02}:{:02}",
                dt.year(),
                dt.month(),
                dt.day(),
                dt.hour(),
                dt.minute()
            ),
            None => write!(f, "Instant({})", self.0),
        }
    }
}

/// Why a date or duration string could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TimeParseError {
    /// Bad date/time text.
    #[error("invalid date/time `{0}`; expected YYYY-MM-DD or YYYY-MM-DDTHH:MM")]
    Instant(String),
    /// Bad duration text.
    #[error("invalid duration `{0}`; expected e.g. 5d, -2h, 1.5d, 90m")]
    Duration(String),
}

impl FromStr for Instant {
    type Err = TimeParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M") {
            return Ok(Self::from_datetime(dt));
        }
        if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S") {
            return Ok(Self::from_datetime(dt));
        }
        if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            return Ok(Self::from_date(d));
        }
        Err(TimeParseError::Instant(s.to_owned()))
    }
}

/// Signed amount of working time in minutes.
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct WorkMinutes(pub i64);

impl WorkMinutes {
    /// No working time.
    pub const ZERO: WorkMinutes = WorkMinutes(0);

    /// `h` hours of working time.
    pub fn hours(h: i64) -> Self {
        Self(h * MINUTES_PER_HOUR)
    }

    /// `d` working days of `minutes_per_day` minutes each.
    pub fn days(d: i64, minutes_per_day: i64) -> Self {
        Self(d * minutes_per_day)
    }

    /// True for a negative amount (a lead).
    pub fn is_negative(self) -> bool {
        self.0 < 0
    }

    /// Absolute value.
    pub fn abs(self) -> Self {
        Self(self.0.abs())
    }

    /// Parses `"5d"`, `"-2h"`, `"1.5d"`, `"90m"`, `"0"`. Fractions round half
    /// away from zero to the nearest minute.
    pub fn parse(s: &str, minutes_per_day: i64) -> Result<Self, TimeParseError> {
        let err = || TimeParseError::Duration(s.to_owned());
        let t = s.trim();
        if t == "0" {
            return Ok(Self::ZERO);
        }
        let (num, unit) = t.split_at(t.len().checked_sub(1).ok_or_else(err)?);
        let factor = match unit {
            "d" | "D" => minutes_per_day,
            "h" | "H" => MINUTES_PER_HOUR,
            "m" | "M" => 1,
            _ => return Err(err()),
        };
        let num = num.trim();
        if !num
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+'))
        {
            return Err(err());
        }
        let value = Decimal::from_str(num).map_err(|_| err())?;
        let minutes = (value * Decimal::from(factor))
            .round_dp_with_strategy(0, RoundingStrategy::MidpointAwayFromZero)
            .to_i64()
            .ok_or_else(err)?;
        Ok(Self(minutes))
    }

    /// Shortest exact representation: whole days, else whole hours, else minutes.
    pub fn format(self, minutes_per_day: i64) -> String {
        let m = self.0;
        if m == 0 {
            "0d".to_owned()
        } else if minutes_per_day > 0 && m % minutes_per_day == 0 {
            format!("{}d", m / minutes_per_day)
        } else if m % MINUTES_PER_HOUR == 0 {
            format!("{}h", m / MINUTES_PER_HOUR)
        } else {
            format!("{m}m")
        }
    }

    /// Working days as a float, for reporting only.
    #[allow(clippy::cast_precision_loss)]
    pub fn as_days_f64(self, minutes_per_day: i64) -> f64 {
        self.0 as f64 / minutes_per_day as f64
    }
}

impl Add for WorkMinutes {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}
impl Sub for WorkMinutes {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0)
    }
}
impl Neg for WorkMinutes {
    type Output = Self;
    fn neg(self) -> Self {
        Self(-self.0)
    }
}
impl AddAssign for WorkMinutes {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}
impl SubAssign for WorkMinutes {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
    }
}
impl std::iter::Sum for WorkMinutes {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        Self(iter.map(|w| w.0).sum())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const DAY8: i64 = 8 * 60;

    #[test]
    fn core_02_epoch_and_display() {
        assert_eq!(Instant::from_ymd_hm(2000, 1, 1, 0, 0).unwrap(), Instant(0));
        assert_eq!(
            Instant::from_ymd_hm(2000, 1, 2, 8, 30).unwrap(),
            Instant(1440 + 510)
        );
        assert_eq!(Instant(-1).day_index(), -1);
        assert_eq!(Instant(-1).minute_of_day(), 1439);
        assert_eq!(
            Instant::from_ymd_hm(2026, 10, 23, 17, 5)
                .unwrap()
                .to_string(),
            "2026-10-23T17:05"
        );
        assert_eq!(
            "2026-10-23".parse::<Instant>().unwrap(),
            Instant::from_ymd_hm(2026, 10, 23, 0, 0).unwrap()
        );
        assert_eq!(
            "2026-10-23T08:00".parse::<Instant>().unwrap(),
            Instant::from_ymd_hm(2026, 10, 23, 8, 0).unwrap()
        );
        assert!("23/10/2026".parse::<Instant>().is_err());
    }

    #[test]
    fn core_02_duration_parse() {
        let p = |s| WorkMinutes::parse(s, DAY8).unwrap().0;
        assert_eq!(p("5d"), 5 * DAY8);
        assert_eq!(p("-2h"), -120);
        assert_eq!(p("1.5d"), 720);
        assert_eq!(p("90m"), 90);
        assert_eq!(p("0"), 0);
        assert_eq!(p(" 3H "), 180);
        assert_eq!(p("0.0125h"), 1, "0.75 min rounds to 1");
        assert_eq!(p("-0.0125h"), -1, "half away from zero, symmetric");
        for bad in ["", "d", "5x", "five d", "5", "1e3d"] {
            assert!(WorkMinutes::parse(bad, DAY8).is_err(), "{bad}");
        }
    }

    #[test]
    fn core_02_duration_format() {
        assert_eq!(WorkMinutes(5 * DAY8).format(DAY8), "5d");
        assert_eq!(WorkMinutes(720).format(DAY8), "12h");
        assert_eq!(WorkMinutes(-90).format(DAY8), "-90m");
        assert_eq!(WorkMinutes(0).format(DAY8), "0d");
    }

    proptest! {
        #[test]
        fn core_02_format_parse_roundtrip(m in -1_000_000i64..1_000_000, mpd in 1i64..1441) {
            let w = WorkMinutes(m);
            prop_assert_eq!(WorkMinutes::parse(&w.format(mpd), mpd).unwrap(), w);
        }

        #[test]
        fn core_02_instant_datetime_roundtrip(m in -50_000_000i64..50_000_000) {
            let i = Instant(m);
            prop_assert_eq!(Instant::from_datetime(i.to_datetime().unwrap()), i);
            prop_assert_eq!(Instant::from_day_index(i.day_index()).0 + i.minute_of_day(), m);
            prop_assert_eq!(i.to_string().parse::<Instant>().unwrap(), i);
        }
    }
}
