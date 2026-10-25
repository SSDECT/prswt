//! APP-04: built-in sample projects, defined in code through the public
//! command catalog (so they also exercise it). All content is original.

use std::sync::Arc;

use prswt_core::{EngineError, Instant};
use prswt_state::ProjectState;
use serde_json::{Value, json};

use crate::{Ctx, FixedClock, Workspace};

/// Names accepted by `sample.load`.
pub const NAMES: [&str; 2] = ["bridge", "turnaround"];

/// Builds sample `name` by running its command script in a scratch workspace.
pub fn build(name: &str) -> Result<ProjectState, EngineError> {
    let start: Instant = match name {
        "bridge" => "2027-03-01T08:00",
        "turnaround" => "2027-05-03T06:00",
        _ => {
            return Err(EngineError::not_found(format!(
                "sample `{name}` (available: {})",
                NAMES.join(", ")
            )));
        }
    }
    .parse()
    .map_err(|_| EngineError::validation("sample start date"))?;
    let mut w = Workspace::new(Arc::new(FixedClock(start)));
    let ctx = Ctx::human("sample");
    let script = match name {
        "bridge" => bridge(),
        _ => turnaround(),
    };
    for (id, params) in script {
        w.execute(&ctx, id, &params)
            .map_err(|e| EngineError::new(e.code, format!("sample step {id}: {}", e.message)))?;
    }
    let repo = w
        .repos
        .into_values()
        .next()
        .ok_or_else(|| EngineError::not_found("sample"))?;
    Ok(repo.branch(repo.live)?.head.clone())
}

fn tri(o: i64, m: i64, p: i64) -> Value {
    json!({ "o": format!("{o}d"), "m": format!("{m}d"), "p": format!("{p}d") })
}

fn bridge() -> Vec<(&'static str, Value)> {
    let mut s = vec![
        (
            "project.create",
            json!({ "name": "Bridge Rehabilitation", "start": "2027-03-01T08:00", "calendar": "5x8" }),
        ),
        (
            "project.update",
            json!({ "must_finish_by": "2027-10-29T17:00" }),
        ),
        (
            "wbs.add",
            json!({ "code": "DES", "name": "Design & permits" }),
        ),
        ("wbs.add", json!({ "code": "PRC", "name": "Procurement" })),
        ("wbs.add", json!({ "code": "CIV", "name": "Civil works" })),
        ("wbs.add", json!({ "code": "CLS", "name": "Closeout" })),
        ("calendar.exception", json!({ "date": "2027-05-31" })),
        ("calendar.exception", json!({ "date": "2027-07-05" })),
        ("calendar.exception", json!({ "date": "2027-09-06" })),
    ];
    let acts: [(&str, &str, &str, Value, &[&str]); 14] = [
        ("M0", "Notice to proceed", "DES", json!(null), &[]),
        ("D10", "Site survey", "DES", json!("5d"), &["M0"]),
        ("D20", "Structural design", "DES", tri(10, 15, 25), &["D10"]),
        (
            "D30",
            "Permit approval",
            "DES",
            json!({ "o": "10d", "m": "20d", "p": "40d", "dist": "pert" }),
            &["D20"],
        ),
        ("P10", "Steel fabrication", "PRC", tri(20, 25, 40), &["D20"]),
        ("P20", "Steel delivery", "PRC", json!("3d"), &["P10"]),
        ("C10", "Mobilize", "CIV", json!("3d"), &["D30"]),
        ("C20", "Demolition", "CIV", tri(8, 10, 15), &["C10"]),
        ("C30", "Foundations", "CIV", tri(12, 15, 22), &["C20"]),
        (
            "C40",
            "Steel erection",
            "CIV",
            tri(10, 12, 18),
            &["C30", "P20"],
        ),
        ("C50", "Deck pour & cure", "CIV", json!("8d"), &["C40"]),
        ("C60", "Paving & marking", "CIV", json!("5d"), &["C50"]),
        ("X10", "Final inspection", "CLS", json!("3d"), &["C60"]),
        ("M9", "Substantial completion", "CLS", json!(null), &["X10"]),
    ];
    for (code, name, wbs, dur, preds) in acts {
        let mut p = json!({ "code": code, "name": name, "wbs": wbs, "predecessors": preds });
        if dur.is_null() {
            p["kind"] = json!(if code == "M0" {
                "start_milestone"
            } else {
                "finish_milestone"
            });
        } else {
            p["duration"] = dur;
        }
        s.push(("activity.add", p));
    }
    s.extend([
        ("resource.add", json!({ "code": "CREW", "name": "Civil crew", "max_units": 2, "rate": 85 })),
        ("resource.add", json!({ "code": "CRANE", "name": "Crawler crane", "kind": "non_labor", "max_units": 1, "rate": 220 })),
        ("resource.assign", json!({ "activity": "C20", "resource": "CREW", "units_per_hour": 2 })),
        ("resource.assign", json!({ "activity": "C30", "resource": "CREW", "units_per_hour": 2 })),
        ("resource.assign", json!({ "activity": "C40", "resource": "CREW", "units_per_hour": 1 })),
        ("resource.assign", json!({ "activity": "C40", "resource": "CRANE" })),
        ("resource.assign", json!({ "activity": "C50", "resource": "CREW", "units_per_hour": 2 })),
        ("resource.assign", json!({ "activity": "D20", "resource": "CREW", "units_per_hour": 1 })),
        ("expense.add", json!({ "activity": "P10", "name": "Structural steel", "amount": 450_000, "accrual": "start" })),
        ("expense.add", json!({ "activity": "C50", "name": "Concrete", "amount": 120_000 })),
        ("risk.add", json!({ "code": "R1", "title": "Permit review takes longer", "probability": 0.35,
            "impacts": [{ "delay": tri(5, 10, 20) }], "bindings": ["D30"] })),
        ("risk.add", json!({ "code": "R2", "title": "Fabrication shop backlog", "probability": 0.25,
            "impacts": [{ "delay": tri(5, 10, 15) }, { "cost": 50_000 }], "bindings": ["P10"] })),
        ("risk.add", json!({ "code": "R3", "title": "Unforeseen ground conditions", "probability": 0.3,
            "impacts": [{ "duration_factor": { "o": 1.1, "m": 1.3, "p": 1.6 } }, { "cost": { "o": 20_000, "m": 40_000, "p": 90_000 } }],
            "bindings": ["C30"] })),
        ("risk.add", json!({ "code": "R4", "title": "Severe weather during civil works", "probability": 0.5,
            "impacts": [{ "delay": tri(1, 3, 6), "once": true }], "bindings": [{ "wbs": "CIV" }] })),
        ("risk.trigger", json!({ "risk": "R3", "target": "R4", "probability": 0.7 })),
        ("risk.correlate", json!({ "members": ["C20", "C30"], "rho": 0.6 })),
    ]);
    s
}

fn turnaround() -> Vec<(&'static str, Value)> {
    let mut s = vec![
        (
            "project.create",
            json!({ "name": "Unit 2 Turnaround", "start": "2027-05-03T06:00", "calendar": "7x24" }),
        ),
        (
            "project.update",
            json!({ "must_finish_by": "2027-05-24T06:00" }),
        ),
        ("wbs.add", json!({ "code": "SD", "name": "Shutdown" })),
        ("wbs.add", json!({ "code": "MX", "name": "Maintenance" })),
        ("wbs.add", json!({ "code": "SU", "name": "Start-up" })),
    ];
    let h = |o: i64, m: i64, p: i64| json!({ "o": format!("{o}h"), "m": format!("{m}h"), "p": format!("{p}h") });
    let acts: [(&str, &str, &str, Value, &[&str]); 10] = [
        ("S10", "Feed out & cool down", "SD", h(18, 24, 36), &[]),
        (
            "S20",
            "Decontaminate & isolate",
            "SD",
            h(24, 30, 48),
            &["S10"],
        ),
        (
            "M10",
            "Open vessels & inspect",
            "MX",
            h(36, 48, 72),
            &["S20"],
        ),
        (
            "M20",
            "Exchanger bundle pull & clean",
            "MX",
            h(48, 60, 96),
            &["S20"],
        ),
        ("M30", "Column tray repairs", "MX", h(72, 96, 160), &["M10"]),
        (
            "M40",
            "Valve & instrument overhaul",
            "MX",
            h(60, 72, 110),
            &["S20"],
        ),
        (
            "M50",
            "Box up & leak test",
            "MX",
            h(24, 30, 48),
            &["M30", "M20", "M40"],
        ),
        (
            "U10",
            "Purge & pressure test",
            "SU",
            h(12, 16, 24),
            &["M50"],
        ),
        ("U20", "Heat up & start-up", "SU", h(18, 24, 40), &["U10"]),
        ("U30", "On spec product", "SU", json!(null), &["U20"]),
    ];
    for (code, name, wbs, dur, preds) in acts {
        let mut p = json!({ "code": code, "name": name, "wbs": wbs, "predecessors": preds });
        if dur.is_null() {
            p["kind"] = json!("finish_milestone");
        } else {
            p["duration"] = dur;
        }
        s.push(("activity.add", p));
    }
    s.extend([
        (
            "resource.add",
            json!({ "code": "FITTERS", "name": "Pipefitters", "max_units": 12, "rate": 95 }),
        ),
        (
            "resource.assign",
            json!({ "activity": "M20", "resource": "FITTERS", "units_per_hour": 6 }),
        ),
        (
            "resource.assign",
            json!({ "activity": "M30", "resource": "FITTERS", "units_per_hour": 8 }),
        ),
        (
            "resource.assign",
            json!({ "activity": "M40", "resource": "FITTERS", "units_per_hour": 4 }),
        ),
        (
            "risk.add",
            json!({ "code": "T1", "title": "Tray damage worse than expected", "probability": 0.4,
            "impacts": [{ "delay": h(24, 48, 96) }], "bindings": ["M30"] }),
        ),
        (
            "risk.add",
            json!({ "code": "T2", "title": "Spare bundle late", "probability": 0.2,
            "impacts": [{ "delay": h(24, 72, 120) }], "bindings": ["M20"] }),
        ),
        (
            "risk.add",
            json!({ "code": "T3", "title": "Start-up trip", "probability": 0.3,
            "impacts": [{ "delay": h(8, 16, 36) }], "bindings": ["U20"] }),
        ),
        (
            "risk.correlate",
            json!({ "members": ["M10", "M30"], "rho": 0.5 }),
        ),
    ]);
    s
}
