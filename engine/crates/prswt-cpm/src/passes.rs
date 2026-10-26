//! CPM-03..13: forward pass, backward pass, floats, ALAP and LOE handling.
//! Formulas: `vault/02-Architecture/03-Pipeline.md` §A; conventions and
//! oracle: `vault/05-Math/CPM-oracles.md`.
//!
//! Passes take a [`Variant`] so Monte Carlo can re-run them with sampled
//! durations and contingent branches switched on or off, without recompiling.

use prswt_calendar::{CalendarError, WorkCalendar};
use prswt_core::model::{ActivityKind, ConstraintKind, ProgressMode, RelationKind, TotalFloatMode};
use prswt_core::{Instant, WorkMinutes};
use serde::Serialize;

use crate::model::{Edge, ScheduleModel, Status};

/// Per-run overrides of the compiled model.
#[derive(Debug, Clone, Copy)]
pub struct Variant<'a> {
    /// Remaining duration per node (working minutes).
    pub dur: &'a [i64],
    /// Which nodes take part (dormant contingent branches are `false`).
    pub node_active: &'a [bool],
    /// Which edges take part.
    pub edge_active: &'a [bool],
}

/// What determined an activity's early start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "edge")]
pub enum Driver {
    /// The project's planned start.
    ProjectStart,
    /// The data date (work cannot be forecast in the past).
    DataDate,
    /// A relationship (edge index).
    Link(usize),
    /// A date constraint.
    Constraint,
    /// Resource leveling held the start back.
    Leveling,
    /// Actual dates (in progress or complete).
    Actual,
    /// Not scheduled (inactive or a WBS summary).
    Inactive,
}

/// Result vectors aligned with `model.nodes` / `model.edges`.
#[derive(Debug, Clone, PartialEq)]
pub struct Dates {
    /// Early starts.
    pub es: Vec<Instant>,
    /// Early finishes.
    pub ef: Vec<Instant>,
    /// Late starts.
    pub ls: Vec<Instant>,
    /// Late finishes.
    pub lf: Vec<Instant>,
    /// Total float (working minutes).
    pub tf: Vec<Option<i64>>,
    /// Free float (working minutes).
    pub ff: Vec<Option<i64>>,
    /// What drove each early start.
    pub driver: Vec<Driver>,
    /// Working-minute slack of each edge on the predecessor calendar
    /// (`None` when the edge doesn't constrain, e.g. inactive or into LOE).
    pub edge_slack: Vec<Option<i64>>,
    /// Latest early finish of any driving activity.
    pub project_finish: Instant,
    /// Late end used for open ends: `must_finish_by` or the project finish.
    pub late_end: Instant,
    /// Remaining duration each node was scheduled with (working minutes):
    /// the variant's, or the work up to an expected finish.
    pub remaining: Vec<i64>,
}

impl ScheduleModel {
    /// Default variant: compiled durations, everything active.
    pub fn base_durations(&self) -> Vec<i64> {
        self.nodes.iter().map(|n| n.dur).collect()
    }

    /// Full deterministic calculation for one variant.
    pub fn run(&self, v: Variant<'_>) -> Result<Dates, CalendarError> {
        let mut d = Dates {
            es: vec![self.planned_start; self.nodes.len()],
            ef: vec![self.planned_start; self.nodes.len()],
            ls: vec![self.planned_start; self.nodes.len()],
            lf: vec![self.planned_start; self.nodes.len()],
            tf: vec![None; self.nodes.len()],
            ff: vec![None; self.nodes.len()],
            driver: vec![Driver::Inactive; self.nodes.len()],
            edge_slack: vec![None; self.edges.len()],
            project_finish: self.planned_start,
            late_end: self.planned_start,
            remaining: v.dur.to_vec(),
        };
        self.forward(v, &mut d)?;
        self.finish_loe(v, &mut d)?;
        d.project_finish = self.project_finish(v, &d);
        d.late_end = self.must_finish_by.unwrap_or(d.project_finish);
        self.backward(v, &mut d)?;
        self.floats(v, &mut d)?;
        self.apply_alap(v, &mut d)?;
        Ok(d)
    }

    fn forward(&self, v: Variant<'_>, d: &mut Dates) -> Result<(), CalendarError> {
        let start = self.planned_start.max(self.data_date);
        for &j in &self.topo {
            let n = &self.nodes[j];
            if !v.node_active[j] || !n.is_network() {
                continue;
            }
            let cal = self.cal(j);
            let dur = WorkMinutes(v.dur[j]);
            match n.status {
                Status::Completed => {
                    d.es[j] = n.actual_start.unwrap_or(start);
                    d.ef[j] = n.actual_finish.unwrap_or(d.es[j]);
                    d.driver[j] = Driver::Actual;
                }
                Status::InProgress => {
                    d.es[j] = n.actual_start.unwrap_or(start);
                    // Suspended work cannot be forecast before its resume date.
                    let mut rem_start = self.data_date.max(n.resume.unwrap_or(self.data_date));
                    if self.settings.progress_mode == ProgressMode::RetainedLogic {
                        self.each_link_candidate(v, d, j, dur, |_, c| {
                            rem_start = rem_start.max(c);
                        })?;
                    }
                    let s = cal.snap_start(rem_start)?;
                    d.ef[j] = cal.add_work(s, dur)?;
                    d.driver[j] = Driver::Actual;
                    self.apply_expected_finish(d, j, s)?;
                }
                Status::NotStarted => {
                    self.forward_not_started(v, d, j, dur, cal, start)?;
                    let es = d.es[j];
                    self.apply_expected_finish(d, j, es)?;
                }
            }
        }
        Ok(())
    }

    /// P6 expected finish: the remaining duration becomes the work between
    /// the remaining start and the expected finish (zero when it has passed).
    fn apply_expected_finish(
        &self,
        d: &mut Dates,
        j: usize,
        rem_start: Instant,
    ) -> Result<(), CalendarError> {
        let Some(target) = self.nodes[j].expected_finish else {
            return Ok(());
        };
        let cal = self.cal(j);
        let work = cal.work_between(rem_start, target)?.0.max(0);
        d.remaining[j] = work;
        d.ef[j] = cal.add_work(rem_start, WorkMinutes(work))?;
        Ok(())
    }

    fn forward_not_started(
        &self,
        v: Variant<'_>,
        d: &mut Dates,
        j: usize,
        dur: WorkMinutes,
        cal: &WorkCalendar,
        start: Instant,
    ) -> Result<(), CalendarError> {
        let n = &self.nodes[j];
        let mut cand = start;
        let mut driver = if self.data_date > self.planned_start {
            Driver::DataDate
        } else {
            Driver::ProjectStart
        };
        self.each_link_candidate(v, d, j, dur, |e, c| {
            if c > cand {
                cand = c;
                driver = Driver::Link(e);
            }
        })?;
        for c in &n.constraints {
            let Some(date) = c.date else { continue };
            let bound = match c.kind {
                ConstraintKind::StartNoEarlierThan | ConstraintKind::StartOn => Some(date),
                ConstraintKind::FinishNoEarlierThan | ConstraintKind::FinishOn => {
                    Some(cal.sub_work(date, dur)?)
                }
                _ => None,
            };
            if let Some(b) = bound
                && b > cand
            {
                cand = b;
                driver = Driver::Constraint;
            }
        }
        // Forward only: late dates ignore leveling, so the delay uses float (P6).
        if let Some(l) = n.leveled_start
            && l > cand
        {
            cand = l;
            driver = Driver::Leveling;
        }
        let mut es = if n.kind == ActivityKind::FinishMilestone {
            cand
        } else {
            cal.snap_start(cand)?
        };
        let mut ef = cal.add_work(es, dur)?;
        for c in &n.constraints {
            match (c.kind, c.date) {
                (ConstraintKind::MandatoryStart, Some(date)) => {
                    es = date;
                    ef = cal.add_work(es, dur)?;
                    driver = Driver::Constraint;
                }
                (ConstraintKind::MandatoryFinish, Some(date)) => {
                    ef = date;
                    es = cal.sub_work(date, dur)?;
                    driver = Driver::Constraint;
                }
                _ => {}
            }
        }
        d.es[j] = es;
        d.ef[j] = ef;
        d.driver[j] = driver;
        Ok(())
    }

    /// Calls `f(edge, candidate)` for every early-start candidate imposed on
    /// `j` by its driving predecessors (no allocation: this is the hot loop).
    fn each_link_candidate(
        &self,
        v: Variant<'_>,
        d: &Dates,
        j: usize,
        dur: WorkMinutes,
        mut f: impl FnMut(usize, Instant),
    ) -> Result<(), CalendarError> {
        let cal = self.cal(j);
        for &e in self.in_edges(j) {
            let edge = &self.edges[e];
            let i = edge.pred;
            if !v.edge_active[e] || !v.node_active[i] || !self.nodes[i].is_driving() {
                continue;
            }
            let lc = &self.calendars[edge.lag_cal];
            let lag = WorkMinutes(edge.lag);
            let c = match edge.kind {
                RelationKind::FS => lc.add_work(d.ef[i], lag)?,
                RelationKind::SS => lc.add_work(d.es[i], lag)?,
                RelationKind::FF => cal.sub_work(lc.add_work(d.ef[i], lag)?, dur)?,
                RelationKind::SF => cal.sub_work(lc.add_work(d.es[i], lag)?, dur)?,
            };
            f(e, c);
        }
        Ok(())
    }

    /// LOE: finishes when its successors need it to (FS → their start, FF →
    /// their finish). Reverse topological order handles LOE chains.
    fn finish_loe(&self, v: Variant<'_>, d: &mut Dates) -> Result<(), CalendarError> {
        for &j in self.topo.iter().rev() {
            if !v.node_active[j] || self.nodes[j].kind != ActivityKind::LevelOfEffort {
                continue;
            }
            let mut ef = d.ef[j].max(d.es[j]);
            for &e in self.out_edges(j) {
                let edge = &self.edges[e];
                if !v.edge_active[e] || !v.node_active[edge.succ] {
                    continue;
                }
                let lc = &self.calendars[edge.lag_cal];
                let lag = WorkMinutes(edge.lag);
                let need = match edge.kind {
                    RelationKind::FS => Some(lc.sub_work(d.es[edge.succ], lag)?),
                    RelationKind::FF => Some(lc.sub_work(d.ef[edge.succ], lag)?),
                    RelationKind::SS | RelationKind::SF => None,
                };
                if let Some(t) = need {
                    ef = ef.max(t);
                }
            }
            d.ef[j] = ef;
            d.ls[j] = d.es[j];
            d.lf[j] = ef;
        }
        Ok(())
    }

    fn project_finish(&self, v: Variant<'_>, d: &Dates) -> Instant {
        (0..self.nodes.len())
            .filter(|&j| v.node_active[j] && self.nodes[j].is_driving())
            .map(|j| d.ef[j])
            .max()
            .unwrap_or(self.planned_start)
    }

    fn backward(&self, v: Variant<'_>, d: &mut Dates) -> Result<(), CalendarError> {
        for &i in self.topo.iter().rev() {
            let n = &self.nodes[i];
            if !v.node_active[i] || !n.is_driving() {
                continue;
            }
            if n.status == Status::Completed {
                d.ls[i] = d.es[i];
                d.lf[i] = d.ef[i];
                continue;
            }
            let cal = self.cal(i);
            let dur = WorkMinutes(d.remaining[i]);
            let mut cand = d.late_end;
            for &e in self.out_edges(i) {
                if let Some(c) = self.late_candidate(v, d, e, dur, cal)? {
                    cand = cand.min(c);
                }
            }
            for c in &n.constraints {
                let Some(date) = c.date else { continue };
                let bound = match c.kind {
                    ConstraintKind::StartNoLaterThan | ConstraintKind::StartOn => {
                        Some(cal.add_work(date, dur)?)
                    }
                    ConstraintKind::FinishNoLaterThan | ConstraintKind::FinishOn => Some(date),
                    _ => None,
                };
                if let Some(b) = bound {
                    cand = cand.min(b);
                }
            }
            let mut lf = if n.kind == ActivityKind::StartMilestone {
                cand
            } else {
                cal.snap_finish(cand)?
            };
            let mut ls = cal.sub_work(lf, dur)?;
            for c in &n.constraints {
                match (c.kind, c.date) {
                    (ConstraintKind::MandatoryStart, Some(date)) => {
                        ls = date;
                        lf = cal.add_work(date, dur)?;
                    }
                    (ConstraintKind::MandatoryFinish, Some(date)) => {
                        lf = date;
                        ls = cal.sub_work(date, dur)?;
                    }
                    _ => {}
                }
            }
            d.ls[i] = ls;
            d.lf[i] = lf;
        }
        Ok(())
    }

    /// Latest-finish candidate that edge `e` imposes on its predecessor.
    fn late_candidate(
        &self,
        v: Variant<'_>,
        d: &Dates,
        e: usize,
        dur: WorkMinutes,
        cal: &WorkCalendar,
    ) -> Result<Option<Instant>, CalendarError> {
        let edge = &self.edges[e];
        let k = edge.succ;
        if !self.constrains(v, edge, e) {
            return Ok(None);
        }
        let lc = &self.calendars[edge.lag_cal];
        let lag = WorkMinutes(edge.lag);
        Ok(Some(match edge.kind {
            RelationKind::FS => lc.sub_work(d.ls[k], lag)?,
            RelationKind::SS => cal.add_work(lc.sub_work(d.ls[k], lag)?, dur)?,
            RelationKind::FF => lc.sub_work(d.lf[k], lag)?,
            RelationKind::SF => cal.add_work(lc.sub_work(d.lf[k], lag)?, dur)?,
        }))
    }

    /// Edge participates in late dates / free float: both ends active and
    /// driving, successor not completed.
    fn constrains(&self, v: Variant<'_>, edge: &Edge, e: usize) -> bool {
        v.edge_active[e]
            && v.node_active[edge.pred]
            && v.node_active[edge.succ]
            && self.nodes[edge.pred].is_driving()
            && self.nodes[edge.succ].is_driving()
            && self.nodes[edge.succ].status != Status::Completed
    }

    fn floats(&self, v: Variant<'_>, d: &mut Dates) -> Result<(), CalendarError> {
        for (e, edge) in self.edges.iter().enumerate() {
            if !self.constrains(v, edge, e) {
                continue;
            }
            let (i, k) = (edge.pred, edge.succ);
            let lc = &self.calendars[edge.lag_cal];
            let lag = WorkMinutes(edge.lag);
            let (req, actual) = match edge.kind {
                RelationKind::FS => (lc.add_work(d.ef[i], lag)?, d.es[k]),
                RelationKind::SS => (lc.add_work(d.es[i], lag)?, d.es[k]),
                RelationKind::FF => (lc.add_work(d.ef[i], lag)?, d.ef[k]),
                RelationKind::SF => (lc.add_work(d.es[i], lag)?, d.ef[k]),
            };
            d.edge_slack[e] = Some(self.cal(i).work_between(req, actual)?.0);
        }
        for i in 0..self.nodes.len() {
            let n = &self.nodes[i];
            if !v.node_active[i] || !n.is_driving() || n.status == Status::Completed {
                continue;
            }
            let cal = self.cal(i);
            let start_tf = cal.work_between(d.es[i], d.ls[i])?.0;
            let finish_tf = cal.work_between(d.ef[i], d.lf[i])?.0;
            let tf = match (n.status, self.settings.total_float_mode) {
                (Status::InProgress, _) | (_, TotalFloatMode::Finish) => finish_tf,
                (_, TotalFloatMode::Start) => start_tf,
                (_, TotalFloatMode::Smallest) => start_tf.min(finish_tf),
            };
            let mut ff: Option<i64> = None;
            for &e in self.out_edges(i) {
                if let Some(s) = d.edge_slack[e] {
                    ff = Some(ff.map_or(s, |f| f.min(s)));
                }
            }
            let ff = match ff {
                Some(f) => f,
                None => cal.work_between(d.ef[i], d.late_end)?.0,
            };
            d.tf[i] = Some(tf);
            d.ff[i] = Some(ff.min(tf));
        }
        Ok(())
    }

    /// CPM-09: ALAP activities slide late by their free float.
    ///
    /// Successors go first (reverse topological order) and a slide hands its
    /// slack to the links into the activity, so a chain of ALAP activities
    /// slides together, as P6 does.
    fn apply_alap(&self, v: Variant<'_>, d: &mut Dates) -> Result<(), CalendarError> {
        for &i in self.topo.iter().rev() {
            let n = &self.nodes[i];
            let alap = n
                .constraints
                .iter()
                .any(|c| c.kind == ConstraintKind::AsLateAsPossible);
            if !alap || !v.node_active[i] || n.status != Status::NotStarted {
                continue;
            }
            // Free float as it stands now: a successor that slid has
            // already widened the slack of its links.
            let slack = self
                .out_edges(i)
                .iter()
                .filter_map(|&e| d.edge_slack[e])
                .min();
            let ff = match (slack, d.ff[i], d.tf[i]) {
                (Some(s), _, Some(tf)) => s.min(tf),
                (Some(s), _, None) => s,
                (None, f, _) => f.unwrap_or(0),
            };
            if ff <= 0 {
                continue;
            }
            let cal = self.cal(i);
            let dur = WorkMinutes(d.remaining[i]);
            d.ef[i] = cal.add_work(d.ef[i], WorkMinutes(ff))?;
            d.es[i] = if dur.0 == 0 {
                d.ef[i]
            } else {
                cal.sub_work(d.ef[i], dur)?
            };
            d.ff[i] = Some(0);
            if let Some(t) = d.tf[i].as_mut() {
                *t -= ff;
            }
            for &e in self.out_edges(i) {
                if let Some(s) = d.edge_slack[e].as_mut() {
                    *s -= ff;
                }
            }
            // Predecessors' links into i gain the slack (their FF is not
            // re-derived: P6 likewise reports ALAP effects only on i).
            for &e in self.in_edges(i) {
                if let Some(s) = d.edge_slack[e].as_mut() {
                    *s += ff;
                }
            }
            d.driver[i] = Driver::Constraint;
        }
        Ok(())
    }
}
