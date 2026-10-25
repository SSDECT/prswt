//! Working-time arithmetic over project calendars.
//!
//! A [`CalendarDef`] (plain data from `prswt-core`) is compiled into a
//! [`WorkCalendar`]: a sorted list of absolute working intervals over a
//! horizon, with prefix sums of working minutes. Every query is a binary
//! search, O(log n), exact integer arithmetic. Definitions and the
//! hand-worked oracle: `vault/05-Math/CAL-oracles.md`.

use std::collections::BTreeMap;

use chrono::{Datelike, NaiveDate};
use prswt_core::ids::CalendarId;
use prswt_core::model::{CalendarDef, DayInterval, WorkWeek};
use prswt_core::time::MINUTES_PER_DAY;
use prswt_core::{EngineError, Instant, WorkMinutes};

/// Cap on compiled horizon (hostile input guard): 200 years.
pub const MAX_HORIZON_DAYS: i64 = 200 * 366;

/// Why a calendar could not be built or queried.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CalendarError {
    /// The calendar or one of its bases does not exist.
    #[error("calendar {0} not found")]
    NotFound(CalendarId),
    /// Following `base` links leads back to the calendar.
    #[error("calendar {0}: base calendar chain contains a cycle")]
    BaseCycle(CalendarId),
    /// Neither the calendar nor any base defines a work week.
    #[error("calendar {0}: no work week defined on it or any base")]
    NoWeek(CalendarId),
    /// A day's working intervals are malformed.
    #[error("calendar {calendar}: invalid intervals on {day}: {reason}")]
    InvalidIntervals {
        /// Calendar with the problem.
        calendar: CalendarId,
        /// Weekday name or date.
        day: String,
        /// What is wrong.
        reason: &'static str,
    },
    /// `minutes_per_day` outside 1..=1440.
    #[error("calendar {0}: minutes_per_day must be in 1..=1440")]
    MinutesPerDay(CalendarId),
    /// The compiled horizon contains no working time at all.
    #[error("calendar {0}: no working time inside the horizon")]
    NoWorkingTime(CalendarId),
    /// Requested horizon is longer than [`MAX_HORIZON_DAYS`].
    #[error("calendar {0}: horizon exceeds {MAX_HORIZON_DAYS} days")]
    HorizonTooLarge(CalendarId),
    /// A query fell outside the compiled horizon; recompile with a longer one.
    #[error("calendar {calendar}: {at} is outside the horizon")]
    OutOfHorizon {
        /// Calendar queried.
        calendar: CalendarId,
        /// Instant outside the horizon.
        at: Instant,
    },
}

impl From<CalendarError> for EngineError {
    fn from(e: CalendarError) -> Self {
        let code = match e {
            CalendarError::NotFound(_) => prswt_core::ErrorCode::NotFound,
            CalendarError::OutOfHorizon { .. } => prswt_core::ErrorCode::Limit,
            _ => prswt_core::ErrorCode::Validation,
        };
        EngineError::new(code, e.to_string())
    }
}

/// CAL-01: intervals of one day must be non-empty, inside `0..=1440`, sorted
/// and non-overlapping.
fn validate_day(
    calendar: CalendarId,
    day: &str,
    intervals: &[DayInterval],
) -> Result<(), CalendarError> {
    let bad = |reason| CalendarError::InvalidIntervals {
        calendar,
        day: day.to_owned(),
        reason,
    };
    let mut prev_end = 0u16;
    for (i, iv) in intervals.iter().enumerate() {
        if iv.is_empty() {
            return Err(bad("interval end must be after start"));
        }
        if iv.end > 1440 {
            return Err(bad("interval ends after midnight"));
        }
        if i > 0 && iv.start < prev_end {
            return Err(bad("intervals overlap or are unsorted"));
        }
        prev_end = iv.end;
    }
    Ok(())
}

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// CAL-01: validates all seven weekdays of a work week.
pub fn validate_week(calendar: CalendarId, week: &WorkWeek) -> Result<(), CalendarError> {
    for (d, intervals) in week.days.iter().enumerate() {
        validate_day(calendar, WEEKDAYS[d], intervals)?;
    }
    Ok(())
}

/// Week and exceptions after applying base-calendar inheritance (CAL-02).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedCalendar {
    /// Calendar id.
    pub id: CalendarId,
    /// Effective work week after inheritance.
    pub week: WorkWeek,
    /// Effective exceptions after inheritance (child overrides base per date).
    pub exceptions: BTreeMap<NaiveDate, Vec<DayInterval>>,
    /// Minutes in one working day, for converting `d` units.
    pub minutes_per_day: i64,
}

/// Follows `base` links: the nearest defined week wins; exceptions merge from
/// the root down, so a child's exception on a date replaces its base's.
pub fn resolve(
    id: CalendarId,
    defs: &BTreeMap<CalendarId, CalendarDef>,
) -> Result<ResolvedCalendar, CalendarError> {
    let mut chain = Vec::new();
    let mut cur = Some(id);
    while let Some(c) = cur {
        if chain.contains(&c) {
            return Err(CalendarError::BaseCycle(id));
        }
        let def = defs.get(&c).ok_or(CalendarError::NotFound(c))?;
        chain.push(c);
        cur = def.base;
    }
    let own = &defs[&id];
    if !(1..=MINUTES_PER_DAY).contains(&own.minutes_per_day) {
        return Err(CalendarError::MinutesPerDay(id));
    }
    let week = chain
        .iter()
        .find_map(|c| defs[c].week.clone())
        .ok_or(CalendarError::NoWeek(id))?;
    validate_week(id, &week)?;
    let mut exceptions = BTreeMap::new();
    for c in chain.iter().rev() {
        for (date, ivs) in &defs[c].exceptions {
            validate_day(id, &date.to_string(), ivs)?;
            exceptions.insert(*date, ivs.clone());
        }
    }
    Ok(ResolvedCalendar {
        id,
        week,
        exceptions,
        minutes_per_day: own.minutes_per_day,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Index {
    /// 24/7 with no exceptions: arithmetic is plain addition (CAL-06).
    Continuous,
    Intervals {
        from: Instant,
        to: Instant,
        starts: Vec<i64>,
        ends: Vec<i64>,
        /// `prefix[k]` = working minutes before interval k; `len = n + 1`.
        prefix: Vec<i64>,
    },
}

/// Compiled calendar ready for O(log n) working-time queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkCalendar {
    id: CalendarId,
    minutes_per_day: i64,
    index: Index,
}

impl WorkCalendar {
    /// Builds the interval index for days `[first_day, last_day]` (CAL-03).
    pub fn compile(
        cal: &ResolvedCalendar,
        first_day: NaiveDate,
        last_day: NaiveDate,
    ) -> Result<Self, CalendarError> {
        let full_day = [DayInterval::new(0, 1440)];
        if cal.exceptions.is_empty() && cal.week.days.iter().all(|d| d[..] == full_day) {
            return Ok(Self {
                id: cal.id,
                minutes_per_day: cal.minutes_per_day,
                index: Index::Continuous,
            });
        }
        if (last_day - first_day).num_days() > MAX_HORIZON_DAYS {
            return Err(CalendarError::HorizonTooLarge(cal.id));
        }
        let mut starts: Vec<i64> = Vec::new();
        let mut ends: Vec<i64> = Vec::new();
        let mut day = first_day;
        while day <= last_day {
            let intervals = cal
                .exceptions
                .get(&day)
                .unwrap_or_else(|| &cal.week.days[day.weekday().num_days_from_monday() as usize]);
            let base = Instant::from_date(day).0;
            for iv in intervals {
                let (s, e) = (base + i64::from(iv.start), base + i64::from(iv.end));
                match ends.last_mut() {
                    Some(last) if *last == s => *last = e,
                    _ => {
                        starts.push(s);
                        ends.push(e);
                    }
                }
            }
            match day.succ_opt() {
                Some(next) => day = next,
                None => break,
            }
        }
        if starts.is_empty() {
            return Err(CalendarError::NoWorkingTime(cal.id));
        }
        let mut prefix = Vec::with_capacity(starts.len() + 1);
        let mut acc = 0;
        prefix.push(0);
        for (s, e) in starts.iter().zip(&ends) {
            acc += e - s;
            prefix.push(acc);
        }
        Ok(Self {
            id: cal.id,
            minutes_per_day: cal.minutes_per_day,
            index: Index::Intervals {
                from: Instant::from_date(first_day),
                to: Instant::from_date(last_day).plus_minutes(MINUTES_PER_DAY),
                starts,
                ends,
                prefix,
            },
        })
    }

    /// Resolve + compile in one step.
    pub fn build(
        id: CalendarId,
        defs: &BTreeMap<CalendarId, CalendarDef>,
        first_day: NaiveDate,
        last_day: NaiveDate,
    ) -> Result<Self, CalendarError> {
        Self::compile(&resolve(id, defs)?, first_day, last_day)
    }

    /// Calendar id.
    pub fn id(&self) -> CalendarId {
        self.id
    }

    /// Minutes in one working day.
    pub fn minutes_per_day(&self) -> i64 {
        self.minutes_per_day
    }

    /// True for the 24/7 fast path (plain arithmetic, no horizon).
    pub fn is_continuous(&self) -> bool {
        matches!(self.index, Index::Continuous)
    }

    fn out(&self, at: i64) -> CalendarError {
        CalendarError::OutOfHorizon {
            calendar: self.id,
            at: Instant(at),
        }
    }

    /// Working minutes in `[horizon start, t)`.
    fn cum(&self, t: Instant) -> Result<i64, CalendarError> {
        match &self.index {
            Index::Continuous => Ok(t.0),
            Index::Intervals {
                from,
                to,
                starts,
                ends,
                prefix,
            } => {
                if t < *from || t > *to {
                    return Err(self.out(t.0));
                }
                let k =
                    partition_near(starts.len(), time_guess(*from, *to, starts.len(), t), |k| {
                        starts[k] <= t.0
                    });
                Ok(if k == 0 {
                    0
                } else {
                    prefix[k - 1] + t.0.min(ends[k - 1]) - starts[k - 1]
                })
            }
        }
    }

    /// Whether the minute `[t, t+1)` is working time.
    pub fn is_working(&self, t: Instant) -> Result<bool, CalendarError> {
        Ok(self.cum(t.plus_minutes(1))? > self.cum(t)?)
    }

    /// `|W ∩ [a, b)|`, negative when `b < a`.
    pub fn work_between(&self, a: Instant, b: Instant) -> Result<WorkMinutes, CalendarError> {
        Ok(WorkMinutes(self.cum(b)? - self.cum(a)?))
    }

    /// Moves forward by `d` working minutes (backward when negative). See the
    /// module docs for the finish/start landing convention (CAL-05).
    pub fn add_work(&self, t: Instant, d: WorkMinutes) -> Result<Instant, CalendarError> {
        if d.0 == 0 {
            return Ok(t);
        }
        let Index::Intervals { starts, prefix, .. } = &self.index else {
            return Ok(t.plus_minutes(d.0));
        };
        let target = self.cum(t)? + d.0;
        let n = starts.len();
        if d.0 > 0 {
            // first interval whose cumulative end reaches the target
            let k = partition_near(n, work_guess(prefix, target), |k| prefix[k + 1] < target);
            if k == n {
                return Err(self.out(t.0));
            }
            Ok(Instant(starts[k] + target - prefix[k]))
        } else {
            if target < 0 {
                return Err(self.out(t.0));
            }
            // interval with prefix[k] <= target < prefix[k+1]
            let k = partition_near(n, work_guess(prefix, target), |k| prefix[k + 1] <= target);
            if k == n {
                return Err(self.out(t.0));
            }
            Ok(Instant(starts[k] + target - prefix[k]))
        }
    }

    /// Moves backward by `d` working minutes (`add_work(t, -d)`).
    pub fn sub_work(&self, t: Instant, d: WorkMinutes) -> Result<Instant, CalendarError> {
        self.add_work(t, -d)
    }

    /// Earliest instant `>= t` that begins a working minute (CAL-04).
    pub fn snap_start(&self, t: Instant) -> Result<Instant, CalendarError> {
        let Index::Intervals {
            from,
            to,
            starts,
            ends,
            ..
        } = &self.index
        else {
            return Ok(t);
        };
        if t < *from || t > *to {
            return Err(self.out(t.0));
        }
        let k = partition_near(starts.len(), time_guess(*from, *to, starts.len(), t), |k| {
            starts[k] <= t.0
        });
        if k > 0 && t.0 < ends[k - 1] {
            Ok(t)
        } else if k < starts.len() {
            Ok(Instant(starts[k]))
        } else {
            Err(self.out(t.0))
        }
    }

    /// Latest instant `<= t` that ends a working minute (CAL-04).
    pub fn snap_finish(&self, t: Instant) -> Result<Instant, CalendarError> {
        let Index::Intervals {
            from,
            to,
            starts,
            ends,
            ..
        } = &self.index
        else {
            return Ok(t);
        };
        if t < *from || t > *to {
            return Err(self.out(t.0));
        }
        let k = partition_near(ends.len(), time_guess(*from, *to, ends.len(), t), |k| {
            ends[k] < t.0
        });
        if k < starts.len() && starts[k] < t.0 {
            Ok(t)
        } else if k > 0 {
            Ok(Instant(ends[k - 1]))
        } else {
            Err(self.out(t.0))
        }
    }
}

/// Index guess for instant `t` assuming intervals spread evenly over the
/// horizon (true for weekly patterns), so the search below is ~O(1).
fn time_guess(from: Instant, to: Instant, n: usize, t: Instant) -> usize {
    let span = (to.0 - from.0).max(1);
    let off = (t.0 - from.0).clamp(0, span);
    let n64 = i64::try_from(n).unwrap_or(i64::MAX);
    usize::try_from(i128::from(off) * i128::from(n64) / i128::from(span)).unwrap_or(n)
}

/// Index guess for a cumulative-work target.
fn work_guess(prefix: &[i64], target: i64) -> usize {
    let n = prefix.len().saturating_sub(1);
    let total = prefix.last().copied().unwrap_or(0).max(1);
    let n64 = i64::try_from(n).unwrap_or(i64::MAX);
    let t = target.clamp(0, total);
    usize::try_from(i128::from(t) * i128::from(n64) / i128::from(total)).unwrap_or(n)
}

/// Number of leading `k in 0..n` for which monotone `pred` holds, found by
/// galloping outward from `guess` and then bisecting the bracket.
fn partition_near(n: usize, guess: usize, pred: impl Fn(usize) -> bool) -> usize {
    let g = guess.min(n);
    let (mut lo, mut hi);
    if g < n && pred(g) {
        // answer in (g, n]
        lo = g + 1;
        let mut step = 1;
        loop {
            let probe = lo.saturating_add(step);
            if probe >= n || !pred(probe) {
                hi = probe.min(n);
                break;
            }
            lo = probe + 1;
            step = step.saturating_mul(2);
        }
    } else {
        // answer in [0, g]
        hi = g;
        let mut step = 1;
        loop {
            if hi == 0 {
                return 0;
            }
            let probe = hi.saturating_sub(step);
            if pred(probe) {
                lo = probe + 1;
                break;
            }
            hi = probe;
            step = step.saturating_mul(2);
        }
    }
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if pred(mid) {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

#[cfg(test)]
mod tests;
