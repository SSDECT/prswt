//! MCP resources: semantic context, not raw tables (`04-MCP-Spec.md` §1).
//! URI: `prswt://projects/{project}/b/{branch}/{view}[/{arg}]`.

use prswt_app::Ctx;
use prswt_core::EngineError;
use serde_json::{Value, json};

use crate::Backend;

const VIEWS: &[(&str, &str, &str)] = &[
    (
        "summary",
        "Schedule summary",
        "Finish, float to target, counts and the driving critical path",
    ),
    (
        "critical-path",
        "Float paths",
        "Top float paths (critical first) with dates",
    ),
    (
        "activities",
        "Activity table",
        "All activities with early/late dates and floats",
    ),
    ("health", "Schedule health", "DCMA 14-point checks"),
    (
        "contention",
        "Resource contention",
        "Usage per resource per week and overallocations",
    ),
    (
        "evm",
        "Earned value",
        "PV/EV/AC and indices against the latest baseline",
    ),
    (
        "risks",
        "Risk register",
        "Risks, bindings, expected delay, mitigations",
    ),
    ("cost", "Budget", "Cost rollup by WBS"),
    ("events", "Audit log", "Event history of the branch"),
];

pub fn templates() -> Vec<Value> {
    let mut t: Vec<Value> = VIEWS
        .iter()
        .map(|(v, name, d)| {
            json!({
                "uriTemplate": format!("prswt://projects/{{project}}/b/{{branch}}/{v}"),
                "name": name, "description": d, "mimeType": "application/json",
            })
        })
        .collect();
    t.push(json!({ "uriTemplate": "prswt://projects/{project}/b/{branch}/activities/{activity}", "name": "Activity context",
        "description": "Dates, floats, links with slack, driving chain, risks", "mimeType": "application/json" }));
    t.push(
        json!({ "uriTemplate": "prswt://projects/{project}/diff/{sandbox}", "name": "Sandbox diff",
        "description": "Changes and schedule delta vs live", "mimeType": "application/json" }),
    );
    t.push(json!({ "uriTemplate": "prswt://simulations/{run}", "name": "Simulation", "mimeType": "application/json" }));
    t
}

pub fn list<B: Backend>(b: &mut B, ctx: &Ctx) -> Vec<Value> {
    let mut out = vec![
        json!({ "uri": "prswt://projects", "name": "Projects", "mimeType": "application/json" }),
        json!({ "uri": "prswt://commands", "name": "Engine commands", "mimeType": "application/json" }),
    ];
    let projects = b.call(ctx, "project.list", &json!({})).unwrap_or_default();
    for p in projects["projects"].as_array().into_iter().flatten() {
        let Some(name) = p["name"].as_str() else {
            continue;
        };
        let enc = encode(name);
        for (v, title, d) in VIEWS {
            out.push(json!({
                "uri": format!("prswt://projects/{enc}/b/live/{v}"),
                "name": format!("{name} · {title}"),
                "description": d,
                "mimeType": "application/json",
            }));
        }
    }
    out
}

pub fn read<B: Backend>(b: &mut B, ctx: &Ctx, uri: &str) -> Result<Value, EngineError> {
    let rest = uri
        .strip_prefix("prswt://")
        .ok_or_else(|| EngineError::not_found(format!("resource {uri}")))?;
    let parts: Vec<String> = rest.split('/').map(decode).collect();
    let p: Vec<&str> = parts.iter().map(String::as_str).collect();
    let call = |b: &mut B, cmd: &str, params: Value| b.call(ctx, cmd, &params);
    match p.as_slice() {
        ["projects"] => call(b, "project.list", json!({})),
        ["commands"] => call(b, "commands.list", json!({})),
        ["simulations", run] => call(b, "simulation.get", json!({ "run": run })),
        ["projects", proj, "diff", sb] => {
            call(b, "sandbox.diff", json!({ "project": proj, "head": sb }))
        }
        ["projects", proj, "b", br, "activities", act] => call(
            b,
            "schedule.activity",
            json!({ "project": proj, "branch": br, "activity": act }),
        ),
        ["projects", proj, "b", br, view] => {
            let base = json!({ "project": proj, "branch": br });
            let with = |extra: Value| {
                let mut m = base.clone();
                if let (Some(m), Some(e)) = (m.as_object_mut(), extra.as_object()) {
                    m.extend(e.clone());
                }
                m
            };
            match *view {
                "summary" => call(b, "schedule.run", base),
                "critical-path" => call(b, "schedule.critical_path", with(json!({ "k": 5 }))),
                "activities" => call(b, "schedule.activities", with(json!({ "limit": 1000 }))),
                "health" => call(b, "schedule.health", base),
                "contention" => {
                    let h = call(b, "resource.histogram", with(json!({ "period": "week" })))?;
                    let o = call(b, "resource.overallocation", base)?;
                    Ok(json!({ "usage": h, "overallocations": o["overallocations"] }))
                }
                "evm" => call(b, "evm.report", base),
                "risks" => call(b, "risk.list", base),
                "cost" => call(b, "cost.summary", base),
                "events" => call(b, "project.history", base),
                v => Err(EngineError::not_found(format!("view `{v}`"))),
            }
        }
        _ => Err(EngineError::not_found(format!("resource {uri}"))),
    }
}

fn encode(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c.to_string()
            } else {
                format!("%{:02X}", u32::from(c))
            }
        })
        .collect()
}

fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(h) = s
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(h);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
