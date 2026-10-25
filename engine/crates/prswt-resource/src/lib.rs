//! Resources & cost (RES-01..05, COST-01).
//!
//! Units are spread over an activity's working time with a cumulative curve
//! `F(x)` (x = elapsed share of working time). Bucket amounts are
//! `U·(F(x₁) − F(x₀))`, so buckets sum to `U` exactly (telescoping, Decimal).
//! Oracle: `vault/05-Math/RES-EVM-oracles.md`.

mod level;

use std::collections::{BTreeMap, HashMap};

use chrono::Duration;
use prswt_calendar::{CalendarError, WorkCalendar};
use prswt_core::ids::{ActivityId, CalendarId, CostAccountId, ResourceId, WbsId};
use prswt_core::model::{
    Accrual, Assignment, CalendarDef, Effective, Expense, Resource, SpreadCurve, WbsNode,
};
use prswt_core::time::MINUTES_PER_DAY;
use prswt_core::{EngineError, Instant, Money, WorkMinutes};
use prswt_cpm::{Schedule, Status};
use rust_decimal::Decimal;
use serde::Serialize;

pub use level::{LevelOptions, LevelingResult, PriorityKey, level};

/// Decimal places kept on cumulative spread amounts.
const SPREAD_DP: u32 = 10;

/// Calendar days compiled around the schedule for resource calendars.
const RESOURCE_HORIZON_MARGIN_DAYS: i64 = 400;

/// Everything resource/cost analysis reads.
#[derive(Debug, Clone, Copy)]
pub struct ResourceInput<'a> {
    /// Calculated schedule (dates per activity).
    pub schedule: &'a Schedule,
    /// Resources.
    pub resources: &'a [Resource],
    /// Assignments.
    pub assignments: &'a [Assignment],
    /// Expenses.
    pub expenses: &'a [Expense],
    /// Calendar definitions (for resource calendars).
    pub calendars: &'a BTreeMap<CalendarId, CalendarDef>,
}

/// Histogram bucket size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Period {
    /// Calendar days.
    Day,
    /// ISO weeks, Monday start.
    Week,
}

impl Period {
    /// Bucket key for a day index (days since 2000-01-01, a Saturday).
    pub fn key(self, day: i64) -> i64 {
        match self {
            Self::Day => day,
            // 2000-01-03 (day 2) is a Monday
            Self::Week => (day - 2).div_euclid(7),
        }
    }
}

/// A run of consecutive days where a resource is over capacity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Overallocation {
    /// Resource.
    pub resource: ResourceId,
    /// First day (day index).
    pub from_day: i64,
    /// Last day (day index).
    pub to_day: i64,
    /// Highest daily demand in the run.
    pub peak_required: Decimal,
    /// Lowest daily capacity in the run.
    pub available: Decimal,
}

/// Planned cost totals.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct CostRollup {
    /// Project total.
    pub total: Money,
    /// Per WBS element, including descendants.
    pub by_wbs: BTreeMap<WbsId, Money>,
    /// Per cost account.
    pub by_cost_account: BTreeMap<CostAccountId, Money>,
    /// Per activity.
    pub by_activity: BTreeMap<ActivityId, Money>,
    /// Day index → cost incurred that day (for PV curves).
    pub by_day: BTreeMap<i64, Money>,
}

/// Cumulative share of the work done at elapsed share `x ∈ [0,1]`.
pub fn cum_fraction(curve: SpreadCurve, x: Decimal) -> Decimal {
    let x = x.clamp(Decimal::ZERO, Decimal::ONE);
    let two = Decimal::TWO;
    let half = Decimal::new(5, 1);
    let one_minus = Decimal::ONE - x;
    match curve {
        SpreadCurve::Uniform => x,
        SpreadCurve::FrontLoaded => Decimal::ONE - one_minus * one_minus,
        SpreadCurve::BackLoaded => x * x,
        SpreadCurve::Bell if x <= half => two * x * x,
        SpreadCurve::Bell => Decimal::ONE - two * one_minus * one_minus,
    }
}

/// Spreads `total` over the working time in `[start, finish)` of `cal` by
/// day. Zero working time puts everything on the start day.
pub fn spread_by_day(
    cal: &WorkCalendar,
    start: Instant,
    finish: Instant,
    total: Decimal,
    curve: SpreadCurve,
) -> Result<Vec<(i64, Decimal)>, CalendarError> {
    let work = cal.work_between(start, finish)?.0;
    if work <= 0 || total.is_zero() {
        return Ok(if total.is_zero() {
            Vec::new()
        } else {
            vec![(start.day_index(), total)]
        });
    }
    let w = Decimal::from(work);
    let mut out = Vec::new();
    let mut done = 0i64;
    let mut prev_amount = Decimal::ZERO;
    for day in start.day_index()..=finish.day_index() {
        let lo = Instant::from_day_index(day).max(start);
        let hi = Instant::from_day_index(day + 1).min(finish);
        if hi <= lo {
            continue;
        }
        let in_day = cal.work_between(lo, hi)?.0;
        if in_day == 0 {
            continue;
        }
        done += in_day;
        // Cumulative amount, rounded; the last one is exactly `total`
        // (F(1) = 1), so the bucket differences telescope to `total`.
        let amount = if done == work {
            total
        } else {
            (total * cum_fraction(curve, Decimal::from(done) / w)).round_dp(SPREAD_DP)
        };
        out.push((day, amount - prev_amount));
        prev_amount = amount;
    }
    Ok(out)
}

/// Value effective at `t` (latest entry with `from <= t`).
pub fn effective_at<T: Clone>(entries: &[Effective<T>], t: Instant) -> Option<T> {
    entries
        .iter()
        .filter(|e| e.from <= t)
        .max_by_key(|e| e.from)
        .map(|e| e.value.clone())
}

/// RES-05: duration of a resource-dependent activity = the longest
/// assignment's `units / units_per_hour`, in working minutes.
pub fn resource_dependent_duration(assignments: &[&Assignment]) -> Option<WorkMinutes> {
    use rust_decimal::prelude::ToPrimitive;
    assignments
        .iter()
        .filter(|a| a.units_per_hour > Decimal::ZERO)
        .filter_map(|a| {
            let units = a.remaining_units.unwrap_or(a.budget_units - a.actual_units);
            (units / a.units_per_hour * Decimal::from(60))
                .round()
                .to_i64()
        })
        .max()
        .map(WorkMinutes)
}

/// Compiled resource calendars + lookups shared by histogram, cost and leveling.
#[derive(Debug)]
pub struct ResourceContext<'a> {
    /// The data analysed.
    pub input: ResourceInput<'a>,
    /// Resource id → index into `input.resources`.
    pub by_id: HashMap<ResourceId, usize>,
    /// Compiled resource calendars.
    pub calendars: HashMap<CalendarId, WorkCalendar>,
}

impl<'a> ResourceContext<'a> {
    /// Compiles resource calendars around the schedule's span.
    pub fn new(input: ResourceInput<'a>) -> Result<Self, EngineError> {
        let s = input.schedule;
        let first = s
            .model
            .planned_start
            .min(s.model.data_date)
            .date()
            .unwrap_or_default()
            - Duration::days(RESOURCE_HORIZON_MARGIN_DAYS);
        let last = s.project_finish().date().unwrap_or_default()
            + Duration::days(RESOURCE_HORIZON_MARGIN_DAYS * 3);
        let mut calendars = HashMap::new();
        for r in input.resources {
            if let std::collections::hash_map::Entry::Vacant(slot) = calendars.entry(r.calendar) {
                slot.insert(WorkCalendar::build(
                    r.calendar,
                    input.calendars,
                    first,
                    last,
                )?);
            }
        }
        let by_id = input
            .resources
            .iter()
            .enumerate()
            .map(|(i, r)| (r.id, i))
            .collect();
        Ok(Self {
            input,
            by_id,
            calendars,
        })
    }

    /// Resource by id.
    pub fn resource(&self, id: ResourceId) -> Option<&Resource> {
        self.by_id
            .get(&id)
            .and_then(|&i| self.input.resources.get(i))
    }

    /// Units available on `day`: max units/hour × working hours on the resource calendar.
    pub fn capacity(&self, r: &Resource, day: i64) -> Result<Decimal, CalendarError> {
        let Some(cal) = self.calendars.get(&r.calendar) else {
            return Ok(Decimal::ZERO);
        };
        let start = Instant::from_day_index(day);
        let hours = Decimal::from(
            cal.work_between(start, start.plus_minutes(MINUTES_PER_DAY))?
                .0,
        ) / Decimal::from(60);
        Ok(effective_at(&r.max_units, start).unwrap_or(Decimal::ZERO) * hours)
    }

    /// Remaining-work window of an activity: `[max(ES, data date), EF]`.
    fn window(&self, i: usize) -> Option<(Instant, Instant)> {
        let s = self.input.schedule;
        let n = s.model.nodes.get(i)?;
        if n.status == Status::Completed || !n.is_network() {
            return None;
        }
        let es = s.dates.es[i];
        let start = if n.status == Status::InProgress {
            es.max(s.model.data_date)
        } else {
            es
        };
        Some((start, s.dates.ef[i]))
    }

    /// Remaining units of an assignment spread by day over its activity's window.
    pub fn assignment_usage(&self, a: &Assignment) -> Result<Vec<(i64, Decimal)>, EngineError> {
        let s = self.input.schedule;
        let Some(i) = s.index_of(a.activity) else {
            return Err(EngineError::not_found(format!("activity {}", a.activity)));
        };
        let Some((start, finish)) = self.window(i) else {
            return Ok(Vec::new());
        };
        let units = a.remaining_units.unwrap_or(a.budget_units - a.actual_units);
        Ok(spread_by_day(
            s.model.cal(i),
            start,
            finish,
            units.max(Decimal::ZERO),
            a.curve,
        )?)
    }

    /// RES-03: remaining usage per resource per period.
    pub fn histogram(
        &self,
        period: Period,
    ) -> Result<BTreeMap<ResourceId, BTreeMap<i64, Decimal>>, EngineError> {
        let mut out: BTreeMap<ResourceId, BTreeMap<i64, Decimal>> = BTreeMap::new();
        for a in self.input.assignments {
            let bucket = out.entry(a.resource).or_default();
            for (day, u) in self.assignment_usage(a)? {
                *bucket.entry(period.key(day)).or_insert(Decimal::ZERO) += u;
            }
        }
        Ok(out)
    }

    /// RES-03: days where demand exceeds capacity, merged into runs.
    pub fn overallocations(&self) -> Result<Vec<Overallocation>, EngineError> {
        let mut out = Vec::new();
        for (rid, days) in self.histogram(Period::Day)? {
            let Some(r) = self.resource(rid) else {
                continue;
            };
            let mut run: Option<Overallocation> = None;
            for (&day, &req) in &days {
                let cap = self.capacity(r, day)?;
                if req > cap {
                    match run.as_mut() {
                        Some(o) if o.to_day + 1 == day => {
                            o.to_day = day;
                            o.peak_required = o.peak_required.max(req);
                            o.available = o.available.min(cap);
                        }
                        _ => {
                            out.extend(run.take());
                            run = Some(Overallocation {
                                resource: rid,
                                from_day: day,
                                to_day: day,
                                peak_required: req,
                                available: cap,
                            });
                        }
                    }
                }
            }
            out.extend(run);
        }
        Ok(out)
    }

    /// COST-01: budget cost = Σ units(day) × rate(day) + expenses, rolled up.
    /// Uses *budget* units over the full ES–EF window (the planning view).
    pub fn cost_rollup(&self, wbs: &[WbsNode]) -> Result<CostRollup, EngineError> {
        let s = self.input.schedule;
        let mut roll = CostRollup::default();
        let add =
            |roll: &mut CostRollup, act: usize, ca: Option<CostAccountId>, day: i64, m: Money| {
                let n = &s.model.nodes[act];
                roll.total += m;
                *roll.by_activity.entry(n.id).or_default() += m;
                *roll.by_day.entry(day).or_default() += m;
                if let Some(ca) = ca {
                    *roll.by_cost_account.entry(ca).or_default() += m;
                }
                let mut cur = Some(n.wbs);
                let mut guard = 0;
                while let Some(w) = cur {
                    *roll.by_wbs.entry(w).or_default() += m;
                    cur = wbs.iter().find(|x| x.id == w).and_then(|x| x.parent);
                    guard += 1;
                    if guard > wbs.len() {
                        break;
                    }
                }
            };
        for a in self.input.assignments {
            let Some(i) = s.index_of(a.activity) else {
                continue;
            };
            let Some(r) = self.resource(a.resource) else {
                continue;
            };
            let (es, ef) = (s.dates.es[i], s.dates.ef[i]);
            for (day, units) in spread_by_day(s.model.cal(i), es, ef, a.budget_units, a.curve)? {
                let rates = effective_at(&r.rates, Instant::from_day_index(day));
                let rate = rates
                    .and_then(|rs| rs.get(usize::from(a.rate_type)).copied())
                    .unwrap_or(Money::ZERO);
                add(&mut roll, i, a.cost_account, day, rate * units);
            }
        }
        for x in self.input.expenses {
            let Some(i) = s.index_of(x.activity) else {
                continue;
            };
            let (es, ef) = (s.dates.es[i], s.dates.ef[i]);
            let parts = match x.accrual {
                Accrual::Start => vec![(es.day_index(), x.budget.0)],
                Accrual::End => vec![(ef.day_index(), x.budget.0)],
                Accrual::Uniform => {
                    spread_by_day(s.model.cal(i), es, ef, x.budget.0, SpreadCurve::Uniform)?
                }
            };
            for (day, amount) in parts {
                add(&mut roll, i, x.cost_account, day, Money(amount));
            }
        }
        Ok(roll)
    }
}

#[cfg(test)]
mod tests;
