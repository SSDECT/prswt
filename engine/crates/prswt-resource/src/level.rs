//! RES-04: resource leveling by serial schedule generation.
//!
//! Activities are placed one at a time, highest priority first among those
//! whose driving predecessors are already placed. Each goes at the earliest
//! working-day start that is logic-feasible (from the placed predecessors'
//! leveled dates) and fits the remaining daily capacity of every resource it
//! uses. Leveling only ever delays.

use std::collections::HashMap;

use prswt_core::ids::{ActivityId, ResourceId};
use prswt_core::model::{RelationKind, SpreadCurve};
use prswt_core::{EngineError, Instant, WorkMinutes};
use prswt_cpm::Status;
use rust_decimal::Decimal;
use serde::Serialize;

use crate::{ResourceContext, spread_by_day};

/// Hard stop when searching for a feasible slot (hostile-input guard).
const DEFAULT_MAX_SHIFT_DAYS: i64 = 3650;

/// Ordering rule for choosing the next activity to place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PriorityKey {
    /// Least total float first.
    TotalFloat,
    /// Earliest early start first.
    EarlyStart,
    /// Activity code (stable tie-break).
    Code,
}

/// Leveling options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LevelOptions {
    /// Priority keys, applied in order.
    pub priority: Vec<PriorityKey>,
    /// Never delay an activity beyond its total float.
    pub preserve_float: bool,
    /// Only these resources constrain (empty = all).
    pub resources: Vec<ResourceId>,
    /// Maximum days to search for a feasible slot per activity.
    pub max_shift_days: i64,
}

impl Default for LevelOptions {
    fn default() -> Self {
        Self {
            priority: vec![
                PriorityKey::TotalFloat,
                PriorityKey::EarlyStart,
                PriorityKey::Code,
            ],
            preserve_float: false,
            resources: Vec::new(),
            max_shift_days: DEFAULT_MAX_SHIFT_DAYS,
        }
    }
}

/// Outcome of leveling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LevelingResult {
    /// Leveled start per node.
    pub start: Vec<Instant>,
    /// Leveled finish per node.
    pub finish: Vec<Instant>,
    /// Activities moved later, with the delay in working minutes.
    pub delayed: Vec<(ActivityId, WorkMinutes)>,
    /// Activities that could not be made resource-feasible (left at logic start).
    pub unresolved: Vec<ActivityId>,
    /// Latest leveled finish.
    pub finish_date: Instant,
}

/// Levels resources over a calculated schedule (serial schedule generation).
#[allow(clippy::too_many_lines)]
pub fn level(
    ctx: &ResourceContext<'_>,
    opts: &LevelOptions,
) -> Result<LevelingResult, EngineError> {
    let s = ctx.input.schedule;
    let m = &s.model;
    let d = &s.dates;
    let n = m.nodes.len();

    let mut demand: Vec<Vec<(ResourceId, Decimal)>> = vec![Vec::new(); n];
    for a in ctx.input.assignments {
        if !opts.resources.is_empty() && !opts.resources.contains(&a.resource) {
            continue;
        }
        if let Some(i) = s.index_of(a.activity) {
            let units = a.remaining_units.unwrap_or(a.budget_units - a.actual_units);
            demand[i].push((a.resource, units.max(Decimal::ZERO)));
        }
    }
    let curves: HashMap<ActivityId, SpreadCurve> = ctx
        .input
        .assignments
        .iter()
        .map(|a| (a.activity, a.curve))
        .collect();

    let mut used: HashMap<(ResourceId, i64), Decimal> = HashMap::new();
    let mut start = d.es.clone();
    let mut finish = d.ef.clone();
    let mut placed = vec![false; n];
    let mut pending: Vec<usize> = (0..n)
        .map(|j| {
            m.in_edges(j)
                .iter()
                .filter(|&&e| m.nodes[m.edges[e].pred].is_driving())
                .count()
        })
        .collect();
    let mut delayed = Vec::new();
    let mut unresolved = Vec::new();

    for _ in 0..n {
        let Some(j) = (0..n)
            .filter(|&j| !placed[j] && pending[j] == 0)
            .min_by(|&a, &b| compare(ctx, opts, a, b))
        else {
            break;
        };
        placed[j] = true;
        for &e in m.out_edges(j) {
            let succ = m.edges[e].succ;
            if m.nodes[j].is_driving() {
                pending[succ] = pending[succ].saturating_sub(1);
            }
        }
        let node = &m.nodes[j];
        if !node.is_driving() || node.status != Status::NotStarted {
            continue;
        }
        let cal = m.cal(j);
        let dur = WorkMinutes(node.dur);
        let logic = logic_start(ctx, j, &start, &finish, &placed)?;
        let mut s0 = cal.snap_start(logic.max(d.es[j]))?;
        let curve = curves.get(&node.id).copied().unwrap_or_default();
        let mut fitted = demand[j].is_empty();
        let mut profile = Vec::new();
        for _ in 0..opts.max_shift_days.max(1) {
            if demand[j].is_empty() {
                break;
            }
            let f0 = cal.add_work(s0, dur)?;
            profile.clear();
            let mut fits = true;
            for &(rid, units) in &demand[j] {
                let Some(r) = ctx.resource(rid) else { continue };
                for (day, u) in spread_by_day(cal, s0, f0, units, curve)? {
                    let have = used.get(&(rid, day)).copied().unwrap_or(Decimal::ZERO);
                    if have + u > ctx.capacity(r, day)? {
                        fits = false;
                    }
                    profile.push((rid, day, u));
                }
            }
            let shift = cal.work_between(d.es[j], s0)?.0;
            if opts.preserve_float && d.tf[j].is_some_and(|tf| shift > tf) {
                break;
            }
            if fits {
                fitted = true;
                break;
            }
            s0 = cal.snap_start(Instant::from_day_index(s0.day_index() + 1))?;
        }
        if !fitted {
            unresolved.push(node.id);
            s0 = cal.snap_start(logic.max(d.es[j]))?;
            profile.clear();
            for &(rid, units) in &demand[j] {
                for (day, u) in spread_by_day(cal, s0, cal.add_work(s0, dur)?, units, curve)? {
                    profile.push((rid, day, u));
                }
            }
        }
        for (rid, day, u) in profile.drain(..) {
            *used.entry((rid, day)).or_insert(Decimal::ZERO) += u;
        }
        start[j] = s0;
        finish[j] = cal.add_work(s0, dur)?;
        let moved = cal.work_between(d.es[j], s0)?;
        if moved.0 > 0 {
            delayed.push((node.id, moved));
        }
    }
    let finish_date = (0..n)
        .filter(|&j| m.nodes[j].is_driving())
        .map(|j| finish[j])
        .max()
        .unwrap_or(m.planned_start);
    Ok(LevelingResult {
        start,
        finish,
        delayed,
        unresolved,
        finish_date,
    })
}

fn compare(
    ctx: &ResourceContext<'_>,
    opts: &LevelOptions,
    a: usize,
    b: usize,
) -> std::cmp::Ordering {
    let s = ctx.input.schedule;
    let mut ord = std::cmp::Ordering::Equal;
    for key in &opts.priority {
        ord = ord.then(match key {
            PriorityKey::TotalFloat => s.dates.tf[a]
                .unwrap_or(i64::MAX)
                .cmp(&s.dates.tf[b].unwrap_or(i64::MAX)),
            PriorityKey::EarlyStart => s.dates.es[a].cmp(&s.dates.es[b]),
            PriorityKey::Code => s.model.nodes[a].code.cmp(&s.model.nodes[b].code),
        });
    }
    ord.then(a.cmp(&b))
}

/// Earliest start allowed by already-placed driving predecessors' leveled dates.
fn logic_start(
    ctx: &ResourceContext<'_>,
    j: usize,
    start: &[Instant],
    finish: &[Instant],
    placed: &[bool],
) -> Result<Instant, EngineError> {
    let m = &ctx.input.schedule.model;
    let cal = m.cal(j);
    let dur = WorkMinutes(m.nodes[j].dur);
    let mut t = m.planned_start.max(m.data_date);
    for &e in m.in_edges(j) {
        let edge = &m.edges[e];
        let i = edge.pred;
        if !placed[i] || !m.nodes[i].is_driving() {
            continue;
        }
        let lc = &m.calendars[edge.lag_cal];
        let lag = WorkMinutes(edge.lag);
        let c = match edge.kind {
            RelationKind::FS => lc.add_work(finish[i], lag)?,
            RelationKind::SS => lc.add_work(start[i], lag)?,
            RelationKind::FF => cal.sub_work(lc.add_work(finish[i], lag)?, dur)?,
            RelationKind::SF => cal.sub_work(lc.add_work(start[i], lag)?, dur)?,
        };
        t = t.max(c);
    }
    Ok(t)
}
