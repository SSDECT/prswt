//! Deterministic Critical Path Method engine (Pillar I).
//!
//! `schedule(input)` validates, compiles and runs the forward/backward
//! passes, then derives floats, critical/near-critical sets, float paths and
//! WBS rollups. Lower-level [`ScheduleModel::run`] with a [`Variant`] is the
//! hot path reused by the Monte Carlo engine.

mod analysis;
pub mod dcma;
mod model;
mod passes;

use std::collections::BTreeMap;

use prswt_core::EngineError;
use prswt_core::ids::{ActivityId, WbsId};
use prswt_core::{Instant, WorkMinutes};
use serde::Serialize;

pub use analysis::{DriverStep, FloatPath};
pub use model::{Edge, Node, ScheduleInput, ScheduleModel, Status};
pub use passes::{Dates, Driver, Variant};

/// Calendar spans tried in order when dates fall outside the compiled horizon.
const HORIZON_STEPS_DAYS: [i64; 3] = [15 * 366, 60 * 366, 190 * 366];

/// Scheduled dates and analytics for one activity.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ActivitySchedule {
    /// Activity id.
    pub id: ActivityId,
    /// Activity code.
    pub code: String,
    /// Early start.
    pub early_start: Instant,
    /// Early finish.
    pub early_finish: Instant,
    /// Late start.
    pub late_start: Instant,
    /// Late finish.
    pub late_finish: Instant,
    /// Total float in working minutes; `None` for completed, LOE and summary activities.
    pub total_float: Option<WorkMinutes>,
    /// Free float in working minutes (never above total float).
    pub free_float: Option<WorkMinutes>,
    /// On the critical path by the project's critical definition.
    pub critical: bool,
    /// Within the near-critical threshold but not critical.
    pub near_critical: bool,
    /// What set the early start.
    pub driver: Driver,
}

/// A fully calculated schedule.
#[derive(Debug, Clone)]
pub struct Schedule {
    /// The compiled network.
    pub model: ScheduleModel,
    /// Calculated dates and floats, indexed like `model.nodes`.
    pub dates: Dates,
    /// Critical flag per node.
    pub critical: Vec<bool>,
    /// Near-critical flag per node.
    pub near_critical: Vec<bool>,
    /// Early start/finish span of every WBS element.
    pub wbs_rollup: BTreeMap<WbsId, (Instant, Instant)>,
}

/// Validate, compile and calculate. Grows the calendar horizon automatically.
pub fn schedule(input: &ScheduleInput<'_>) -> Result<Schedule, EngineError> {
    let mut last_err = None;
    for days in HORIZON_STEPS_DAYS {
        let model = ScheduleModel::compile(input, days)?;
        let dur = model.base_durations();
        let node_active = vec![true; model.nodes.len()];
        let edge_active = vec![true; model.edges.len()];
        let v = Variant {
            dur: &dur,
            node_active: &node_active,
            edge_active: &edge_active,
        };
        match model.run(v) {
            Ok(dates) => return Ok(Schedule::assemble(model, dates, input)),
            Err(e) if model::is_horizon_error(&e) => last_err = Some(e),
            Err(e) => return Err(e.into()),
        }
    }
    Err(last_err.map_or_else(
        || EngineError::new(prswt_core::ErrorCode::Limit, "schedule horizon exhausted"),
        EngineError::from,
    ))
}

impl Schedule {
    fn assemble(model: ScheduleModel, dates: Dates, input: &ScheduleInput<'_>) -> Self {
        let critical = analysis::critical_set(&model, &dates);
        let near_critical = analysis::near_critical_set(&model, &dates, &critical);
        let wbs_rollup = analysis::wbs_rollup(&model, &dates, input.wbs);
        Self {
            model,
            dates,
            critical,
            near_critical,
            wbs_rollup,
        }
    }

    /// Latest early finish of any driving activity.
    pub fn project_finish(&self) -> Instant {
        self.dates.project_finish
    }

    /// Node index of an activity id.
    pub fn index_of(&self, id: ActivityId) -> Option<usize> {
        self.model.index.get(&id).copied()
    }

    /// Node index of an activity code.
    pub fn index_of_code(&self, code: &str) -> Option<usize> {
        self.model.nodes.iter().position(|n| n.code == code)
    }

    /// Dates and analytics of node `i` (WBS summaries report their rollup).
    pub fn activity(&self, i: usize) -> Option<ActivitySchedule> {
        let n = self.model.nodes.get(i)?;
        let d = &self.dates;
        let (es, ef) = if n.kind == prswt_core::model::ActivityKind::WbsSummary {
            self.wbs_rollup
                .get(&n.wbs)
                .copied()
                .unwrap_or((d.es[i], d.ef[i]))
        } else {
            (d.es[i], d.ef[i])
        };
        Some(ActivitySchedule {
            id: n.id,
            code: n.code.clone(),
            early_start: es,
            early_finish: ef,
            late_start: d.ls[i],
            late_finish: d.lf[i],
            total_float: d.tf[i].map(WorkMinutes),
            free_float: d.ff[i].map(WorkMinutes),
            critical: self.critical[i],
            near_critical: self.near_critical[i],
            driver: d.driver[i],
        })
    }

    /// Rows for every activity.
    pub fn activities(&self) -> Vec<ActivitySchedule> {
        (0..self.model.nodes.len())
            .filter_map(|i| self.activity(i))
            .collect()
    }

    /// CPM-15: up to `k` total-float paths, most critical first.
    pub fn float_paths(&self, k: usize) -> Vec<FloatPath> {
        analysis::float_paths(&self.model, &self.dates, k)
    }

    /// CPM-16: chain of what drove activity `i`'s early start, back to its root cause.
    pub fn explain(&self, i: usize) -> Vec<DriverStep> {
        analysis::explain(&self.model, &self.dates, i)
    }
}

#[cfg(test)]
mod tests;
