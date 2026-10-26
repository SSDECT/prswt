//! CPM-12/14/15/16: critical sets, multiple float paths, driver explanation,
//! WBS rollup; CPM-19: out-of-sequence progress (P6 schedule log).

use std::collections::{BTreeMap, HashMap};

use prswt_core::Instant;
use prswt_core::ids::WbsId;
use prswt_core::model::{CriticalDefinition, RelationKind, WbsNode};
use serde::Serialize;

use crate::model::{ScheduleModel, Status};
use crate::passes::{Dates, Driver};

/// Upper bound on driver-chain length (guards against corrupt input).
const MAX_EXPLAIN_STEPS: usize = 100_000;

/// One total-float path (P6 "multiple float paths").
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FloatPath {
    /// Node indices in chronological order.
    pub activities: Vec<usize>,
    /// Smallest total float on the path (working minutes).
    pub float: i64,
}

/// One step of a driver chain: what set an activity's early start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DriverStep {
    /// Node index.
    pub activity: usize,
    /// What drove it.
    pub driver: Driver,
}

/// A link whose successor has progressed further than the link allows
/// (P6 "out-of-sequence"): e.g. an FS successor started before its
/// predecessor finished. Lags are ignored, as in P6's schedule log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutOfSequence {
    /// Edge index in `model.edges`.
    pub edge: usize,
    /// Predecessor node index.
    pub pred: usize,
    /// Successor node index.
    pub succ: usize,
    /// Link type.
    pub kind: RelationKind,
}

/// CPM-19: every link broken by actual dates, in edge order.
pub fn out_of_sequence(m: &ScheduleModel) -> Vec<OutOfSequence> {
    m.edges
        .iter()
        .enumerate()
        .filter_map(|(e, edge)| {
            let (p, s) = (&m.nodes[edge.pred], &m.nodes[edge.succ]);
            // (successor's actual that the link governs, predecessor's actual it needs)
            let (succ_actual, pred_actual) = match edge.kind {
                RelationKind::FS => (s.actual_start, p.actual_finish),
                RelationKind::SS => (s.actual_start, p.actual_start),
                RelationKind::FF => (s.actual_finish, p.actual_finish),
                RelationKind::SF => (s.actual_finish, p.actual_start),
            };
            let broken = succ_actual.is_some_and(|t| pred_actual.is_none_or(|need| need > t));
            broken.then_some(OutOfSequence {
                edge: e,
                pred: edge.pred,
                succ: edge.succ,
                kind: edge.kind,
            })
        })
        .collect()
}

fn eligible(m: &ScheduleModel, d: &Dates, i: usize) -> bool {
    let n = &m.nodes[i];
    n.is_driving() && n.status != Status::Completed && d.tf[i].is_some()
}

pub fn critical_set(m: &ScheduleModel, d: &Dates) -> Vec<bool> {
    let n = m.nodes.len();
    match m.settings.critical {
        CriticalDefinition::TotalFloatAtMost(x) => (0..n)
            .map(|i| eligible(m, d, i) && d.tf[i].is_some_and(|t| t <= x.0))
            .collect(),
        CriticalDefinition::LongestPath => {
            let mut crit = vec![false; n];
            let mut stack: Vec<usize> = (0..n)
                .filter(|&i| eligible(m, d, i) && d.ef[i] == d.project_finish)
                .collect();
            while let Some(i) = stack.pop() {
                if crit[i] {
                    continue;
                }
                crit[i] = true;
                for &e in m.in_edges(i) {
                    let p = m.edges[e].pred;
                    if d.edge_slack[e] == Some(0) && eligible(m, d, p) && !crit[p] {
                        stack.push(p);
                    }
                }
            }
            crit
        }
    }
}

pub fn near_critical_set(m: &ScheduleModel, d: &Dates, critical: &[bool]) -> Vec<bool> {
    let limit = m.settings.near_critical_threshold.0;
    (0..m.nodes.len())
        .map(|i| !critical[i] && eligible(m, d, i) && d.tf[i].is_some_and(|t| t <= limit))
        .collect()
}

/// Multiple float paths: seed with the unassigned activity of least total
/// float (latest early finish, then code, breaks ties), walk backward through
/// the unassigned predecessor with least link slack, then least float.
pub fn float_paths(m: &ScheduleModel, d: &Dates, k: usize) -> Vec<FloatPath> {
    let n = m.nodes.len();
    let mut assigned = vec![false; n];
    let mut paths = Vec::new();
    while paths.len() < k {
        let seed = (0..n)
            .filter(|&i| !assigned[i] && eligible(m, d, i))
            .min_by(|&a, &b| {
                d.tf[a]
                    .cmp(&d.tf[b])
                    .then(d.ef[b].cmp(&d.ef[a]))
                    .then(m.nodes[a].code.cmp(&m.nodes[b].code))
            });
        let Some(seed) = seed else { break };
        let mut path = vec![seed];
        assigned[seed] = true;
        let mut cur = seed;
        loop {
            let next = m
                .in_edges(cur)
                .iter()
                .filter_map(|&e| {
                    let p = m.edges[e].pred;
                    let slack = d.edge_slack[e]?;
                    (!assigned[p] && eligible(m, d, p)).then_some((slack, p))
                })
                .min_by(|a, b| {
                    a.0.cmp(&b.0)
                        .then(d.tf[a.1].cmp(&d.tf[b.1]))
                        .then(m.nodes[a.1].code.cmp(&m.nodes[b.1].code))
                });
            let Some((_, p)) = next else { break };
            assigned[p] = true;
            path.push(p);
            cur = p;
        }
        path.reverse();
        let float = path.iter().filter_map(|&i| d.tf[i]).min().unwrap_or(0);
        paths.push(FloatPath {
            activities: path,
            float,
        });
    }
    paths
}

pub fn explain(m: &ScheduleModel, d: &Dates, i: usize) -> Vec<DriverStep> {
    let mut steps = Vec::new();
    let mut cur = i;
    while cur < m.nodes.len() && steps.len() < MAX_EXPLAIN_STEPS {
        let driver = d.driver[cur];
        steps.push(DriverStep {
            activity: cur,
            driver,
        });
        match driver {
            Driver::Link(e) => cur = m.edges[e].pred,
            _ => break,
        }
    }
    steps
}

/// Early-date span of every WBS node over all descendant network activities.
pub fn wbs_rollup(
    m: &ScheduleModel,
    d: &Dates,
    wbs: &[WbsNode],
) -> BTreeMap<WbsId, (Instant, Instant)> {
    let parent: HashMap<WbsId, Option<WbsId>> = wbs.iter().map(|w| (w.id, w.parent)).collect();
    let mut span: BTreeMap<WbsId, (Instant, Instant)> = BTreeMap::new();
    for (i, n) in m.nodes.iter().enumerate() {
        if !n.is_network() {
            continue;
        }
        let mut cur = Some(n.wbs);
        let mut guard = 0usize;
        while let Some(w) = cur {
            let entry = span.entry(w).or_insert((d.es[i], d.ef[i]));
            entry.0 = entry.0.min(d.es[i]);
            entry.1 = entry.1.max(d.ef[i]);
            cur = parent.get(&w).copied().flatten();
            guard += 1;
            if guard > wbs.len() {
                break; // corrupt (cyclic) WBS: stop climbing
            }
        }
    }
    span
}
