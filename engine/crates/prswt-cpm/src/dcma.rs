//! CPM-17: DCMA 14-point schedule assessment.
//!
//! Thresholds follow the public DCMA 14-Point Assessment guidance. Checks
//! that need data the caller doesn't have (e.g. no baseline) report
//! `pass: None` rather than guessing.

use std::collections::{HashMap, HashSet};

use prswt_core::ids::ActivityId;
use prswt_core::model::{ConstraintKind, RelationKind};
use prswt_core::{Instant, WorkMinutes};
use serde::Serialize;

use crate::Schedule;
use crate::model::Status;
use crate::passes::Variant;

const HIGH_FLOAT_DAYS: i64 = 44;
const HIGH_DURATION_DAYS: i64 = 44;
const CP_TEST_DELAY_DAYS: i64 = 600;
const PCT_LIMIT: f64 = 5.0;

/// Result of a DCMA 14-point assessment.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DcmaReport {
    /// The 14 checks in order.
    pub checks: Vec<DcmaCheck>,
    /// Passed / evaluated checks.
    pub score: f64,
}

/// One DCMA check.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DcmaCheck {
    /// Check number 1..=14.
    pub number: u8,
    /// Check name.
    pub name: &'static str,
    /// Measured value (percentage, count or index), when computable.
    pub value: Option<f64>,
    /// Pass threshold as text.
    pub threshold: &'static str,
    /// Pass/fail; `None` when it could not be evaluated.
    pub pass: Option<bool>,
    /// Activities or links that fail the check.
    pub offenders: Vec<String>,
}

/// Extra facts the network alone doesn't carry.
#[derive(Debug, Clone, Default)]
pub struct DcmaContext {
    /// Activities with at least one resource or cost assignment.
    pub resourced: HashSet<ActivityId>,
    /// Baseline finish per activity, when a baseline exists.
    pub baseline_finish: Option<HashMap<ActivityId, Instant>>,
}

/// Runs all 14 checks against a calculated schedule.
#[allow(clippy::too_many_lines, clippy::cast_precision_loss)]
pub fn assess(s: &Schedule, ctx: &DcmaContext) -> DcmaReport {
    let m = &s.model;
    let d = &s.dates;
    let incomplete: Vec<usize> = (0..m.nodes.len())
        .filter(|&i| m.nodes[i].is_driving() && m.nodes[i].status != Status::Completed)
        .collect();
    let total = incomplete.len().max(1) as f64;
    let links = m.edges.len().max(1) as f64;
    let pct = |count: usize, of: f64| count as f64 * 100.0 / of;
    let code = |i: usize| m.nodes[i].code.clone();
    let days = |i: usize, n: i64| n * m.cal(i).minutes_per_day();
    let mut checks = Vec::new();
    let mut push = |number, name, value: Option<f64>, threshold, pass, offenders| {
        checks.push(DcmaCheck {
            number,
            name,
            value,
            threshold,
            pass,
            offenders,
        });
    };

    // 1 Logic: missing predecessor or successor (project start/finish milestones excused).
    let missing: Vec<String> = incomplete
        .iter()
        .copied()
        .filter(|&i| {
            let no_pred = m.in_edges(i).is_empty() && !m.nodes[i].kind.is_milestone();
            let no_succ = m.out_edges(i).is_empty() && !m.nodes[i].kind.is_milestone();
            no_pred || no_succ
        })
        .map(code)
        .collect();
    let v = pct(missing.len(), total);
    push(1, "Logic", Some(v), "<= 5%", Some(v <= PCT_LIMIT), missing);

    // 2 Leads (negative lags) and 3 Lags.
    let leads: Vec<String> = m
        .edges
        .iter()
        .filter(|e| e.lag < 0)
        .map(|e| format!("{} -> {}", m.nodes[e.pred].code, m.nodes[e.succ].code))
        .collect();
    push(
        2,
        "Leads",
        Some(leads.len() as f64),
        "= 0",
        Some(leads.is_empty()),
        leads,
    );
    let lags: Vec<String> = m
        .edges
        .iter()
        .filter(|e| e.lag > 0)
        .map(|e| format!("{} -> {}", m.nodes[e.pred].code, m.nodes[e.succ].code))
        .collect();
    let v = pct(lags.len(), links);
    push(3, "Lags", Some(v), "<= 5%", Some(v <= PCT_LIMIT), lags);

    // 4 Relationship types: FS share.
    let fs = m
        .edges
        .iter()
        .filter(|e| e.kind == RelationKind::FS)
        .count();
    let v = if m.edges.is_empty() {
        100.0
    } else {
        pct(fs, links)
    };
    let non_fs = m
        .edges
        .iter()
        .filter(|e| e.kind != RelationKind::FS)
        .map(|e| {
            format!(
                "{} -{:?}-> {}",
                m.nodes[e.pred].code, e.kind, m.nodes[e.succ].code
            )
        })
        .collect();
    push(
        4,
        "Relationship types",
        Some(v),
        ">= 90% FS",
        Some(v >= 90.0),
        non_fs,
    );

    // 5 Hard constraints.
    let hard: Vec<String> = incomplete
        .iter()
        .copied()
        .filter(|&i| {
            m.nodes[i].constraints.iter().any(|c| {
                matches!(
                    c.kind,
                    ConstraintKind::MandatoryStart
                        | ConstraintKind::MandatoryFinish
                        | ConstraintKind::StartNoLaterThan
                        | ConstraintKind::FinishNoLaterThan
                )
            })
        })
        .map(code)
        .collect();
    let v = pct(hard.len(), total);
    push(
        5,
        "Hard constraints",
        Some(v),
        "<= 5%",
        Some(v <= PCT_LIMIT),
        hard,
    );

    // 6 High float, 7 Negative float.
    let high: Vec<String> = incomplete
        .iter()
        .copied()
        .filter(|&i| d.tf[i].is_some_and(|t| t > days(i, HIGH_FLOAT_DAYS)))
        .map(code)
        .collect();
    let v = pct(high.len(), total);
    push(
        6,
        "High float",
        Some(v),
        "<= 5% over 44d",
        Some(v <= PCT_LIMIT),
        high,
    );
    let neg: Vec<String> = incomplete
        .iter()
        .copied()
        .filter(|&i| d.tf[i].is_some_and(|t| t < 0))
        .map(code)
        .collect();
    push(
        7,
        "Negative float",
        Some(neg.len() as f64),
        "= 0",
        Some(neg.is_empty()),
        neg,
    );

    // 8 High duration.
    let long: Vec<String> = incomplete
        .iter()
        .copied()
        .filter(|&i| m.nodes[i].dur > days(i, HIGH_DURATION_DAYS))
        .map(code)
        .collect();
    let v = pct(long.len(), total);
    push(
        8,
        "High duration",
        Some(v),
        "<= 5% over 44d",
        Some(v <= PCT_LIMIT),
        long,
    );

    // 9 Invalid dates: actuals after the data date, forecasts before it.
    let dd = m.data_date;
    let invalid: Vec<String> = (0..m.nodes.len())
        .filter(|&i| {
            let n = &m.nodes[i];
            let actual_future =
                n.actual_start.is_some_and(|t| t > dd) || n.actual_finish.is_some_and(|t| t > dd);
            let forecast_past = n.status == Status::NotStarted && n.is_driving() && d.es[i] < dd;
            actual_future || forecast_past
        })
        .map(code)
        .collect();
    push(
        9,
        "Invalid dates",
        Some(invalid.len() as f64),
        "= 0",
        Some(invalid.is_empty()),
        invalid,
    );

    // 10 Resources.
    let unres: Vec<String> = incomplete
        .iter()
        .copied()
        .filter(|&i| m.nodes[i].dur > 0 && !ctx.resourced.contains(&m.nodes[i].id))
        .map(code)
        .collect();
    let v = pct(unres.len(), total);
    push(
        10,
        "Resources",
        Some(v),
        "<= 5% unresourced",
        Some(v <= PCT_LIMIT),
        unres,
    );

    // 11 Missed tasks and 14 BEI need a baseline.
    if let Some(bl) = &ctx.baseline_finish {
        let due: Vec<usize> = (0..m.nodes.len())
            .filter(|&i| bl.get(&m.nodes[i].id).is_some_and(|&f| f <= dd))
            .collect();
        let missed: Vec<String> = due
            .iter()
            .copied()
            .filter(|&i| {
                let n = &m.nodes[i];
                match (n.actual_finish, bl.get(&n.id)) {
                    (Some(af), Some(&bf)) => af > bf,
                    _ => true,
                }
            })
            .map(code)
            .collect();
        let of = due.len().max(1) as f64;
        let v = pct(missed.len(), of);
        push(
            11,
            "Missed tasks",
            Some(v),
            "<= 5%",
            Some(v <= PCT_LIMIT),
            missed,
        );
        let done = due
            .iter()
            .filter(|&&i| m.nodes[i].status == Status::Completed)
            .count();
        let bei = if due.is_empty() {
            1.0
        } else {
            done as f64 / of
        };
        push(
            14,
            "BEI",
            Some(bei),
            ">= 0.95",
            Some(bei >= 0.95),
            Vec::new(),
        );
    } else {
        push(11, "Missed tasks", None, "<= 5%", None, Vec::new());
        push(14, "BEI", None, ">= 0.95", None, Vec::new());
    }

    // 12 Critical path test.
    let (cp_pass, cp_off) = critical_path_test(s);
    push(
        12,
        "Critical path test",
        None,
        "finish moves with delay",
        cp_pass,
        cp_off,
    );

    // 13 CPLI.
    let pc = &m.calendars[m.project_cal];
    let cp_len = pc
        .work_between(dd.max(m.planned_start), d.project_finish)
        .map_or(0, |w| w.0);
    let end_float = m
        .must_finish_by
        .and_then(|t| pc.work_between(d.project_finish, t).ok())
        .map_or(0, |w| w.0);
    let cpli = if cp_len > 0 {
        Some((cp_len + end_float) as f64 / cp_len as f64)
    } else {
        None
    };
    push(
        13,
        "CPLI",
        cpli,
        ">= 0.95",
        cpli.map(|c| c >= 0.95),
        Vec::new(),
    );

    checks.sort_by_key(|c| c.number);
    let evaluated = checks.iter().filter(|c| c.pass.is_some()).count();
    let passed = checks.iter().filter(|c| c.pass == Some(true)).count();
    DcmaReport {
        score: if evaluated == 0 {
            0.0
        } else {
            passed as f64 / evaluated as f64
        },
        checks,
    }
}

/// Delay the first critical, unfinished activity by 600 working days; the
/// project finish must move by at least that much (minus one day tolerance
/// for calendar snapping across mixed calendars).
fn critical_path_test(s: &Schedule) -> (Option<bool>, Vec<String>) {
    let m = &s.model;
    let Some(&i) = m
        .topo
        .iter()
        .find(|&&i| s.critical[i] && m.nodes[i].status != Status::Completed)
    else {
        return (None, Vec::new());
    };
    let cal = m.cal(i);
    let delay = CP_TEST_DELAY_DAYS * cal.minutes_per_day();
    let mut dur = m.base_durations();
    dur[i] += delay;
    let na = vec![true; m.nodes.len()];
    let ea = vec![true; m.edges.len()];
    let v = Variant {
        dur: &dur,
        node_active: &na,
        edge_active: &ea,
    };
    let Ok(after) = m.run(v) else {
        return (None, Vec::new());
    };
    let moved = cal
        .work_between(s.dates.project_finish, after.project_finish)
        .unwrap_or(WorkMinutes::ZERO)
        .0;
    let ok = moved >= delay - cal.minutes_per_day();
    (
        Some(ok),
        if ok {
            Vec::new()
        } else {
            vec![m.nodes[i].code.clone()]
        },
    )
}
