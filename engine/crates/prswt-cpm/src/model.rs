//! CPM-01/02: compile editable entities into a dense, validated, topologically
//! ordered [`ScheduleModel`].

use std::collections::{BTreeMap, HashMap, HashSet};

use chrono::{Duration, NaiveDate};
use prswt_calendar::{CalendarError, ResolvedCalendar, WorkCalendar};
use prswt_core::ids::{ActivityId, CalendarId, RelationshipId, WbsId};
use prswt_core::model::{
    Activity, ActivityKind, CalendarDef, Constraint, LagCalendar, PercentCompleteType, Project,
    RelationKind, Relationship, ScheduleSettings, WbsNode, WorkWeek,
};
use prswt_core::{EngineError, ErrorCode, Instant};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

/// Days of calendar compiled before the earliest project date (room for negative lags).
const HORIZON_LEAD_DAYS: i64 = 400;

/// Everything the scheduler reads. Borrowed, so callers keep ownership.
#[derive(Debug, Clone, Copy)]
pub struct ScheduleInput<'a> {
    /// Project header (dates, settings, default calendar).
    pub project: &'a Project,
    /// Calendar definitions by id.
    pub calendars: &'a BTreeMap<CalendarId, CalendarDef>,
    /// WBS elements (for rollups).
    pub wbs: &'a [WbsNode],
    /// Activities to schedule.
    pub activities: &'a [Activity],
    /// Logic links.
    pub relationships: &'a [Relationship],
}

/// Progress state of an activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// No actual start.
    NotStarted,
    /// Started, not finished.
    InProgress,
    /// Actually finished.
    Completed,
}

/// One activity in the dense network.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// Activity id.
    pub id: ActivityId,
    /// Activity code.
    pub code: String,
    /// WBS element.
    pub wbs: WbsId,
    /// Activity type.
    pub kind: ActivityKind,
    /// Index into `ScheduleModel::calendars`.
    pub cal: usize,
    /// Working minutes still to schedule (remaining duration).
    pub dur: i64,
    /// Primary and secondary constraints.
    pub constraints: Vec<Constraint>,
    /// Progress state.
    pub status: Status,
    /// Actual start.
    pub actual_start: Option<Instant>,
    /// Actual finish.
    pub actual_finish: Option<Instant>,
    /// Expected finish (P6), when the project uses it; sets the remaining duration.
    pub expected_finish: Option<Instant>,
    /// Resume date of a suspended in-progress activity.
    pub resume: Option<Instant>,
    /// Leveled start (not-started activities only).
    pub leveled_start: Option<Instant>,
}

impl Node {
    /// Takes part in the forward/backward passes (WBS summaries are rolled up instead).
    pub fn is_network(&self) -> bool {
        self.kind != ActivityKind::WbsSummary
    }

    /// Can drive successors and receive late dates from successors.
    pub fn is_driving(&self) -> bool {
        !matches!(
            self.kind,
            ActivityKind::LevelOfEffort | ActivityKind::WbsSummary
        )
    }
}

/// One relationship in the dense network.
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    /// Source relationship id (`None` for engine-generated links).
    pub id: Option<RelationshipId>,
    /// Predecessor node index.
    pub pred: usize,
    /// Successor node index.
    pub succ: usize,
    /// Link type.
    pub kind: RelationKind,
    /// Lag in working minutes.
    pub lag: i64,
    /// Index of the calendar the lag is measured on.
    pub lag_cal: usize,
}

/// Dense, validated network ready for repeated passes.
#[derive(Debug, Clone)]
pub struct ScheduleModel {
    /// Nodes, one per activity.
    pub nodes: Vec<Node>,
    /// Edges, one per relationship.
    pub edges: Vec<Edge>,
    /// CSR: out-edges of node `n` are `succ_edges[succ_off[n]..succ_off[n + 1]]`.
    pub succ_off: Vec<usize>,
    /// Out-edge ids grouped by node (see `succ_off`).
    pub succ_edges: Vec<usize>,
    /// CSR offsets: in-edges of node `n` are `pred_edges[pred_off[n]..pred_off[n + 1]]`.
    pub pred_off: Vec<usize>,
    /// In-edge ids grouped by node.
    pub pred_edges: Vec<usize>,
    /// Nodes in topological order (predecessors first).
    pub topo: Vec<usize>,
    /// Compiled calendars; the last is the 24/7 calendar used for 24h lags.
    pub calendars: Vec<WorkCalendar>,
    /// Index of the project calendar.
    pub project_cal: usize,
    /// Activity id → node index.
    pub index: HashMap<ActivityId, usize>,
    /// Status date.
    pub data_date: Instant,
    /// Planned project start.
    pub planned_start: Instant,
    /// Target finish, if any.
    pub must_finish_by: Option<Instant>,
    /// Scheduling options.
    pub settings: ScheduleSettings,
}

impl ScheduleModel {
    /// Validates and compiles. `horizon_days` is the calendar span compiled
    /// after the earliest relevant date.
    pub fn compile(input: &ScheduleInput<'_>, horizon_days: i64) -> Result<Self, EngineError> {
        let project = input.project;
        let mut cal_ids: Vec<CalendarId> = vec![project.default_calendar];
        for a in input.activities {
            a.validate()?;
            if !cal_ids.contains(&a.calendar) {
                cal_ids.push(a.calendar);
            }
        }
        let first_day = earliest_day(input) - Duration::days(HORIZON_LEAD_DAYS);
        let last_day = first_day + Duration::days(horizon_days);
        let mut calendars = Vec::with_capacity(cal_ids.len() + 1);
        for id in &cal_ids {
            calendars.push(WorkCalendar::build(
                *id,
                input.calendars,
                first_day,
                last_day,
            )?);
        }
        let cal24 = calendars.len();
        calendars.push(continuous_calendar(project.default_calendar, &calendars)?);
        let cal_index: HashMap<CalendarId, usize> =
            cal_ids.iter().enumerate().map(|(i, c)| (*c, i)).collect();

        let mut index = HashMap::with_capacity(input.activities.len());
        let mut nodes = Vec::with_capacity(input.activities.len());
        for a in input.activities {
            if index.insert(a.id, nodes.len()).is_some() {
                return Err(EngineError::validation(format!(
                    "duplicate activity id {}",
                    a.id
                )));
            }
            nodes.push(node_from(
                a,
                cal_index.get(&a.calendar).copied().unwrap_or(0),
                project.settings.use_expected_finish,
            ));
        }

        let mut edges = Vec::with_capacity(input.relationships.len());
        let mut seen = HashSet::new();
        for r in input.relationships {
            let pred = *index
                .get(&r.pred)
                .ok_or_else(|| EngineError::not_found(format!("predecessor {}", r.pred)))?;
            let succ = *index
                .get(&r.succ)
                .ok_or_else(|| EngineError::not_found(format!("successor {}", r.succ)))?;
            if pred == succ {
                return Err(EngineError::validation(format!(
                    "activity {} cannot depend on itself",
                    nodes[pred].code
                )));
            }
            if !seen.insert((pred, succ, r.kind)) {
                return Err(EngineError::validation(format!(
                    "duplicate {:?} link {} -> {}",
                    r.kind, nodes[pred].code, nodes[succ].code
                )));
            }
            if !nodes[pred].is_network() || !nodes[succ].is_network() {
                return Err(EngineError::validation(
                    "WBS summary activities cannot have relationships",
                ));
            }
            let policy = r.lag_calendar.unwrap_or(project.settings.lag_calendar);
            let lag_cal = match policy {
                LagCalendar::Predecessor => nodes[pred].cal,
                LagCalendar::Successor => nodes[succ].cal,
                LagCalendar::Project => 0,
                LagCalendar::TwentyFourHour => cal24,
            };
            edges.push(Edge {
                id: Some(r.id),
                pred,
                succ,
                kind: r.kind,
                lag: r.lag.0,
                lag_cal,
            });
        }

        let (succ_off, succ_edges) = csr(nodes.len(), &edges, |e| e.pred);
        let (pred_off, pred_edges) = csr(nodes.len(), &edges, |e| e.succ);
        let mut model = Self {
            nodes,
            edges,
            succ_off,
            succ_edges,
            pred_off,
            pred_edges,
            topo: Vec::new(),
            calendars,
            project_cal: 0,
            index,
            data_date: project.data_date,
            planned_start: project.planned_start,
            must_finish_by: project.must_finish_by,
            settings: project.settings,
        };
        model.topo = model.topological_order()?;
        Ok(model)
    }

    /// Out-edge ids of node `n`.
    pub fn out_edges(&self, n: usize) -> &[usize] {
        &self.succ_edges[self.succ_off[n]..self.succ_off[n + 1]]
    }

    /// In-edge ids of node `n`.
    pub fn in_edges(&self, n: usize) -> &[usize] {
        &self.pred_edges[self.pred_off[n]..self.pred_off[n + 1]]
    }

    /// Calendar of node `n`.
    pub fn cal(&self, n: usize) -> &WorkCalendar {
        &self.calendars[self.nodes[n].cal]
    }

    /// Kahn's algorithm; on failure a DFS extracts one concrete cycle.
    fn topological_order(&self) -> Result<Vec<usize>, EngineError> {
        let n = self.nodes.len();
        let mut indegree: Vec<usize> = (0..n).map(|v| self.in_edges(v).len()).collect();
        let mut queue: Vec<usize> = (0..n).filter(|&v| indegree[v] == 0).rev().collect();
        let mut order = Vec::with_capacity(n);
        while let Some(v) = queue.pop() {
            order.push(v);
            for &e in self.out_edges(v) {
                let s = self.edges[e].succ;
                indegree[s] -= 1;
                if indegree[s] == 0 {
                    queue.push(s);
                }
            }
        }
        if order.len() == n {
            return Ok(order);
        }
        let cycle = self.find_cycle(&indegree);
        let codes: Vec<&str> = cycle.iter().map(|&v| self.nodes[v].code.as_str()).collect();
        Err(EngineError::new(
            ErrorCode::Cycle,
            format!("relationship loop: {}", codes.join(" -> ")),
        )
        .with_details(serde_json::json!({ "cycle": codes })))
    }

    /// Iterative DFS restricted to nodes left over by Kahn (all lie on or
    /// behind a cycle). Returns the cycle closed on its first node.
    fn find_cycle(&self, indegree: &[usize]) -> Vec<usize> {
        let n = self.nodes.len();
        let remaining: Vec<bool> = indegree.iter().map(|&d| d > 0).collect();
        let mut state = vec![0u8; n]; // 0 new, 1 on stack, 2 done
        for root in (0..n).filter(|&v| remaining[v]) {
            if state[root] != 0 {
                continue;
            }
            let mut stack: Vec<(usize, usize)> = vec![(root, 0)];
            state[root] = 1;
            while let Some(&mut (v, ref mut i)) = stack.last_mut() {
                let outs = self.out_edges(v);
                if let Some(&e) = outs.get(*i) {
                    *i += 1;
                    let s = self.edges[e].succ;
                    if !remaining[s] {
                        continue;
                    }
                    if state[s] == 1 {
                        let pos = stack.iter().position(|&(x, _)| x == s).unwrap_or(0);
                        let mut cycle: Vec<usize> = stack[pos..].iter().map(|&(x, _)| x).collect();
                        cycle.push(s);
                        return cycle;
                    }
                    if state[s] == 0 {
                        state[s] = 1;
                        stack.push((s, 0));
                    }
                } else {
                    state[v] = 2;
                    stack.pop();
                }
            }
        }
        Vec::new()
    }
}

fn node_from(a: &Activity, cal: usize, use_expected_finish: bool) -> Node {
    let status = if a.actual_finish.is_some() {
        Status::Completed
    } else if a.actual_start.is_some() {
        Status::InProgress
    } else {
        Status::NotStarted
    };
    let planned = a.duration.planning_value().0;
    let dur = match status {
        Status::Completed => 0,
        _ if a.kind.is_milestone() => 0,
        Status::InProgress => a.remaining.map_or_else(
            || {
                if a.pct_type == PercentCompleteType::Duration {
                    let left = Decimal::ONE_HUNDRED - a.pct_complete;
                    (Decimal::from(planned) * left / Decimal::ONE_HUNDRED)
                        .round()
                        .to_i64()
                        .unwrap_or(planned)
                } else {
                    planned
                }
            },
            |r| r.0,
        ),
        Status::NotStarted => a.remaining.map_or(planned, |r| r.0),
    };
    let constraints = [a.primary_constraint, a.secondary_constraint]
        .into_iter()
        .flatten()
        .collect();
    Node {
        id: a.id,
        code: a.code.clone(),
        wbs: a.wbs,
        kind: a.kind,
        cal,
        dur: dur.max(0),
        constraints,
        status,
        actual_start: a.actual_start,
        actual_finish: a.actual_finish,
        expected_finish: a.expected_finish.filter(|_| {
            use_expected_finish && status != Status::Completed && !a.kind.is_milestone()
        }),
        resume: a.resume.filter(|_| status == Status::InProgress),
        leveled_start: a.leveled_start.filter(|_| status == Status::NotStarted),
    }
}

fn earliest_day(input: &ScheduleInput<'_>) -> NaiveDate {
    let p = input.project;
    let mut t = p.planned_start.min(p.data_date);
    for a in input.activities {
        for x in [
            a.actual_start,
            a.actual_finish,
            a.resume,
            a.expected_finish,
            a.leveled_start,
        ]
        .into_iter()
        .flatten()
        .chain(
            [a.primary_constraint, a.secondary_constraint]
                .into_iter()
                .flatten()
                .filter_map(|c| c.date),
        ) {
            t = t.min(x);
        }
    }
    t.date().unwrap_or_default()
}

fn continuous_calendar(
    id: CalendarId,
    others: &[WorkCalendar],
) -> Result<WorkCalendar, EngineError> {
    let resolved = ResolvedCalendar {
        id,
        week: WorkWeek::continuous(),
        exceptions: BTreeMap::new(),
        minutes_per_day: others.first().map_or(1440, WorkCalendar::minutes_per_day),
    };
    let day = NaiveDate::default();
    WorkCalendar::compile(&resolved, day, day).map_err(EngineError::from)
}

fn csr(n: usize, edges: &[Edge], key: impl Fn(&Edge) -> usize) -> (Vec<usize>, Vec<usize>) {
    let mut off = vec![0usize; n + 1];
    for e in edges {
        off[key(e) + 1] += 1;
    }
    for i in 0..n {
        off[i + 1] += off[i];
    }
    let mut fill = off.clone();
    let mut list = vec![0usize; edges.len()];
    for (i, e) in edges.iter().enumerate() {
        let k = key(e);
        list[fill[k]] = i;
        fill[k] += 1;
    }
    (off, list)
}

/// True when a calendar error means "grow the horizon and retry".
pub(crate) fn is_horizon_error(e: &CalendarError) -> bool {
    matches!(e, CalendarError::OutOfHorizon { .. })
}
