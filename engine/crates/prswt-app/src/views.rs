//! JSON shapes returned to UI/CLI/MCP. Dates are ISO-like local strings,
//! durations are working-time strings ("5d", "4h") on the activity calendar.

use prswt_core::model::{ActivityKind, Estimate};
use prswt_core::{Instant, WorkMinutes};
use prswt_cpm::Schedule;
use prswt_state::ProjectState;
use serde_json::{Value, json};

use crate::params::days;

pub fn t(i: Instant) -> String {
    i.to_string()
}

pub fn kind_str(k: ActivityKind) -> &'static str {
    match k {
        ActivityKind::TaskDependent => "task",
        ActivityKind::ResourceDependent => "resource_dependent",
        ActivityKind::LevelOfEffort => "level_of_effort",
        ActivityKind::StartMilestone => "start_milestone",
        ActivityKind::FinishMilestone => "finish_milestone",
        ActivityKind::WbsSummary => "wbs_summary",
    }
}

pub fn estimate(e: &Estimate, mpd: i64) -> Value {
    match e {
        Estimate::Deterministic { value } => json!(days(value.0, mpd)),
        Estimate::Triangular { o, m, p } => {
            json!({ "dist": "triangular", "o": days(o.0, mpd), "m": days(m.0, mpd), "p": days(p.0, mpd) })
        }
        Estimate::Pert { o, m, p, .. } => {
            json!({ "dist": "pert", "o": days(o.0, mpd), "m": days(m.0, mpd), "p": days(p.0, mpd) })
        }
        Estimate::Uniform { min, max } => {
            json!({ "dist": "uniform", "o": days(min.0, mpd), "p": days(max.0, mpd) })
        }
        Estimate::Discrete { outcomes } => json!({
            "dist": "discrete",
            "outcomes": outcomes.iter().map(|(v, p)| json!([days(v.0, mpd), p])).collect::<Vec<_>>()
        }),
    }
}

/// One schedule row.
pub fn activity_row(state: &ProjectState, s: &Schedule, i: usize) -> Value {
    let Some(a) = s.activity(i) else {
        return Value::Null;
    };
    let n = &s.model.nodes[i];
    let mpd = s.model.cal(i).minutes_per_day();
    let src = state.activities.get(&a.id);
    let wbs = src
        .and_then(|x| state.wbs.get(&x.wbs))
        .map(|w| w.code.clone());
    json!({
        "id": a.id,
        "code": a.code,
        "name": src.map(|x| x.name.clone()),
        "wbs": wbs,
        "kind": kind_str(n.kind),
        "duration": src.map(|x| estimate(&x.duration, mpd)),
        "remaining": days(n.dur, mpd),
        "status": format!("{:?}", n.status).to_lowercase(),
        "early_start": t(a.early_start),
        "early_finish": t(a.early_finish),
        "late_start": t(a.late_start),
        "late_finish": t(a.late_finish),
        "total_float": a.total_float.map(|w| days(w.0, mpd)),
        "free_float": a.free_float.map(|w| days(w.0, mpd)),
        "total_float_days": a.total_float.map(|w| w.as_days_f64(mpd)),
        "critical": a.critical,
        "near_critical": a.near_critical,
        "constraint": src.and_then(|x| x.primary_constraint).map(|c| json!({
            "kind": c.kind,
            "date": c.date.map(t),
        })),
        "leveled_start": src.and_then(|x| x.leveled_start).map(t),
        "expected_finish": src.and_then(|x| x.expected_finish).map(t),
        "suspend": src.and_then(|x| x.suspend).map(t),
        "resume": src.and_then(|x| x.resume).map(t),
    })
}

pub fn summary(state: &ProjectState, s: &Schedule) -> Value {
    let p = &state.project;
    let pc = &s.model.calendars[s.model.project_cal];
    let mpd = pc.minutes_per_day();
    let span = pc
        .work_between(p.planned_start.max(p.data_date), s.project_finish())
        .map_or(0, |w| w.0);
    let crit = s.critical.iter().filter(|&&c| c).count();
    let near = s.near_critical.iter().filter(|&&c| c).count();
    let end_float = p
        .must_finish_by
        .and_then(|d| pc.work_between(s.project_finish(), d).ok())
        .map(|w: WorkMinutes| days(w.0, mpd));
    json!({
        "project": p.name,
        "project_id": p.id,
        "data_date": t(p.data_date),
        "planned_start": t(p.planned_start),
        "finish": t(s.project_finish()),
        "must_finish_by": p.must_finish_by.map(t),
        "float_to_target": end_float,
        "remaining_duration": days(span, mpd),
        "activities": state.activities.len(),
        "relationships": state.relationships.len(),
        "critical": crit,
        "near_critical": near,
        "risks": state.risks.len(),
        "resources": state.resources.len(),
        "baselines": state.baselines.len(),
    })
}
