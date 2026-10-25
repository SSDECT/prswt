//! Earned Value Management (EVM-01..04).
//!
//! A [`Baseline`] freezes the time-phased budget (cost per day from the
//! resource/cost rollup) and the project calendar. PV at an instant is the
//! sum of whole days before it plus the worked share of its own day.
//! Ratios are `Option`: division by zero gives `None`, never NaN/∞.
//! Oracle: `vault/05-Math/RES-EVM-oracles.md` §EVM.

use std::collections::BTreeMap;

use prswt_calendar::WorkCalendar;
use prswt_core::ids::{ActivityId, BaselineId};
use prswt_core::model::{Activity, Assignment, Expense};
use prswt_core::time::MINUTES_PER_DAY;
use prswt_core::{Instant, Money};
use prswt_cpm::Schedule;
use prswt_resource::CostRollup;
use rust_decimal::Decimal;
use serde::Serialize;

/// Bisection steps for earned schedule (minutes resolution over ~2^40 range).
const ES_SEARCH_STEPS: u32 = 48;

/// Frozen budget plan.
#[derive(Debug, Clone)]
pub struct Baseline {
    /// Baseline id.
    pub id: BaselineId,
    /// Display name.
    pub name: String,
    /// Planned start at capture.
    pub start: Instant,
    /// Planned finish at capture.
    pub finish: Instant,
    /// Budget at completion.
    pub bac: Money,
    /// Budget per day (day index → amount).
    pub by_day: BTreeMap<i64, Money>,
    /// Budget per activity (BAC_i, for earned value).
    pub bac_by_activity: BTreeMap<ActivityId, Money>,
    /// Project calendar at capture (intra-day PV and earned schedule).
    pub calendar: WorkCalendar,
}

/// Estimate-at-completion formula.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EacMethod {
    /// BAC / CPI
    Cpi,
    /// AC + (BAC − EV): remaining work at budget rate.
    RemainingAtBudget,
    /// AC + (BAC − EV) / (CPI · SPI)
    CpiSpi,
}

/// Earned value metrics as of a date.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EvmReport {
    /// Measurement date.
    pub as_of: Instant,
    /// Budget at completion.
    pub bac: Money,
    /// Planned value.
    pub pv: Money,
    /// Earned value.
    pub ev: Money,
    /// Actual cost.
    pub ac: Money,
    /// Cost variance EV − AC.
    pub cv: Money,
    /// Schedule variance EV − PV.
    pub sv: Money,
    /// Cost performance index EV / AC.
    pub cpi: Option<Decimal>,
    /// Schedule performance index EV / PV.
    pub spi: Option<Decimal>,
    /// Estimate at completion.
    pub eac: Option<Money>,
    /// Estimate to complete EAC − AC.
    pub etc: Option<Money>,
    /// Variance at completion BAC − EAC.
    pub vac: Option<Money>,
    /// To-complete performance index (BAC − EV) / (BAC − AC).
    pub tcpi: Option<Decimal>,
    /// Earned schedule: working minutes from baseline start where PV = EV.
    pub earned_schedule: Option<i64>,
    /// Working minutes elapsed from the baseline start to `as_of`.
    pub actual_time: i64,
    /// Time-based schedule performance: earned schedule / actual time.
    pub spi_t: Option<Decimal>,
}

impl Baseline {
    /// EVM-01: capture from a calculated schedule and its cost rollup.
    pub fn capture(
        id: BaselineId,
        name: impl Into<String>,
        s: &Schedule,
        cost: &CostRollup,
    ) -> Self {
        let m = &s.model;
        Self {
            id,
            name: name.into(),
            start: m.planned_start,
            finish: s.project_finish(),
            bac: cost.total,
            by_day: cost.by_day.clone(),
            bac_by_activity: cost.by_activity.clone(),
            calendar: m.calendars[m.project_cal].clone(),
        }
    }

    /// EVM-02: planned value at `t`.
    pub fn pv(&self, t: Instant) -> Money {
        let day = t.day_index();
        let whole: Money = self.by_day.range(..day).map(|(_, m)| *m).sum();
        let Some(&today) = self.by_day.get(&day) else {
            return whole;
        };
        let day_start = Instant::from_day_index(day);
        let worked = self.calendar.work_between(day_start, t).map_or(0, |w| w.0);
        let in_day = self
            .calendar
            .work_between(day_start, day_start.plus_minutes(MINUTES_PER_DAY))
            .map_or(0, |w| w.0);
        if in_day <= 0 || worked >= in_day {
            return if t > day_start { whole + today } else { whole };
        }
        whole + today * (Decimal::from(worked) / Decimal::from(in_day))
    }

    /// Cumulative PV at the end of each day of the baseline.
    pub fn pv_curve(&self) -> Vec<(i64, Money)> {
        let mut acc = Money::ZERO;
        self.by_day
            .iter()
            .map(|(&d, &m)| {
                acc += m;
                (d, acc)
            })
            .collect()
    }

    /// Smallest instant with PV ≥ `ev` (bisection; PV is non-decreasing).
    fn time_of_pv(&self, ev: Money) -> Option<Instant> {
        if ev <= Money::ZERO {
            return Some(self.start);
        }
        let first = *self.by_day.keys().next()?;
        let last = *self.by_day.keys().next_back()?;
        let (mut lo, mut hi) = (
            Instant::from_day_index(first).0,
            Instant::from_day_index(last + 1).0,
        );
        if self.pv(Instant(hi)) < ev {
            return None;
        }
        for _ in 0..ES_SEARCH_STEPS {
            if lo >= hi {
                break;
            }
            let mid = lo + (hi - lo) / 2;
            if self.pv(Instant(mid)) >= ev {
                hi = mid;
            } else {
                lo = mid + 1;
            }
        }
        Some(Instant(hi))
    }
}

/// EVM-02/03/04 as of the current data date.
pub fn evaluate(
    baseline: &Baseline,
    as_of: Instant,
    activities: &[Activity],
    assignments: &[Assignment],
    expenses: &[Expense],
    method: EacMethod,
) -> EvmReport {
    let hundred = Decimal::ONE_HUNDRED;
    let ev: Money = activities
        .iter()
        .map(|a| {
            let bac = baseline
                .bac_by_activity
                .get(&a.id)
                .copied()
                .unwrap_or(Money::ZERO);
            let pct = if a.is_completed() {
                hundred
            } else {
                a.pct_complete.clamp(Decimal::ZERO, hundred)
            };
            bac * (pct / hundred)
        })
        .sum();
    let ac: Money = assignments.iter().map(|a| a.actual_cost).sum::<Money>()
        + expenses.iter().map(|x| x.actual).sum::<Money>();
    let pv = baseline.pv(as_of);
    let bac = baseline.bac;
    let cpi = ratio(ev, ac);
    let spi = ratio(ev, pv);
    let eac = match method {
        EacMethod::Cpi => cpi.and_then(|c| bac.0.checked_div(c)).map(Money),
        EacMethod::RemainingAtBudget => Some(ac + bac - ev),
        EacMethod::CpiSpi => cpi
            .zip(spi)
            .and_then(|(c, s)| (bac - ev).0.checked_div(c * s))
            .map(|x| ac + Money(x)),
    };
    let tcpi = ratio(bac - ev, bac - ac);
    let cal = &baseline.calendar;
    let actual_time = cal.work_between(baseline.start, as_of).map_or(0, |w| w.0);
    let earned_schedule = baseline
        .time_of_pv(ev)
        .and_then(|t| cal.work_between(baseline.start, t).ok())
        .map(|w| w.0);
    let spi_t =
        earned_schedule.and_then(|es| Decimal::from(es).checked_div(Decimal::from(actual_time)));
    EvmReport {
        as_of,
        bac,
        pv,
        ev,
        ac,
        cv: ev - ac,
        sv: ev - pv,
        cpi,
        spi,
        eac,
        etc: eac.map(|e| e - ac),
        vac: eac.map(|e| bac - e),
        tcpi,
        earned_schedule,
        actual_time,
        spi_t,
    }
}

fn ratio(a: Money, b: Money) -> Option<Decimal> {
    if b.0.is_zero() {
        None
    } else {
        a.0.checked_div(b.0)
    }
}

#[cfg(test)]
mod tests;
