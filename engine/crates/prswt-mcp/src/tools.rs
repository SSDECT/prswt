//! MCP-02..09: curated tools (`vault/02-Architecture/04-MCP-Spec.md` §2).
//! Each maps to one catalog command; arguments pass through unchanged.
//! Two generic tools reach everything else: `execute_command`, `list_commands`.

use serde_json::{Map, Value, json};

/// Parameter type, mapped to JSON Schema and checked before a call.
#[derive(Debug, Clone, Copy)]
pub enum Ty {
    /// String.
    Str,
    /// Number (or numeric string).
    Num,
    /// Non-negative integer.
    Int,
    /// Boolean.
    Bool,
    /// "5d", "-2h" …
    Dur,
    /// Duration or {o, m, p, dist}.
    Est,
    /// Array.
    Arr,
    /// Object.
    Obj,
}

/// (name, type, required, description)
pub type Param = (&'static str, Ty, bool, &'static str);

/// One curated MCP tool and the engine command behind it.
#[derive(Debug, Clone, Copy)]
pub struct Tool {
    /// Tool name as agents see it.
    pub name: &'static str,
    /// Catalog command it runs.
    pub command: &'static str,
    /// What the tool does, for the agent.
    pub description: &'static str,
    /// Accepted arguments.
    pub params: &'static [Param],
    /// True when it never changes state (MCP `readOnlyHint`).
    pub read_only: bool,
}

const PROJ: Param = (
    "project",
    Ty::Str,
    false,
    "Project name or id (optional when one project is open)",
);
const BR: Param = (
    "branch",
    Ty::Str,
    false,
    "\"live\" (default) or a sandbox name",
);

/// The curated tool set (`04-MCP-Spec.md` §2).
pub static TOOLS: &[Tool] = &[
    Tool {
        name: "list_projects",
        command: "project.list",
        read_only: true,
        description: "Open projects with finish date and sandbox count.",
        params: &[],
    },
    Tool {
        name: "load_sample",
        command: "sample.load",
        read_only: false,
        description: "Open a built-in sample project (bridge = 5x8 civil works with risks; turnaround = 24/7 refinery outage).",
        params: &[("name", Ty::Str, true, "bridge | turnaround")],
    },
    Tool {
        name: "create_project",
        command: "project.create",
        read_only: false,
        description: "Create an empty project with a standard calendar.",
        params: &[
            ("name", Ty::Str, true, "Project name"),
            ("start", Ty::Str, true, "Planned start YYYY-MM-DD[THH:MM]"),
            ("calendar", Ty::Str, false, "5x8 | 6x10 | 7x24"),
        ],
    },
    Tool {
        name: "get_schedule_summary",
        command: "schedule.run",
        read_only: true,
        description: "Recalculate (F9) and summarise: finish, float to target, counts, driving critical path.",
        params: &[PROJ, BR],
    },
    Tool {
        name: "query_activities",
        command: "schedule.activities",
        read_only: true,
        description: "Activity table with early/late dates and floats; filter and page.",
        params: &[
            PROJ,
            BR,
            ("critical", Ty::Bool, false, "Only critical"),
            ("max_float", Ty::Dur, false, "Only total float <= this"),
            ("text", Ty::Str, false, "Code/name contains"),
            ("wbs", Ty::Str, false, "WBS code"),
            ("sort", Ty::Str, false, "float | start | code"),
            ("limit", Ty::Int, false, "Max rows (<=1000)"),
            ("offset", Ty::Int, false, "Skip rows"),
        ],
    },
    Tool {
        name: "explain_activity_dates",
        command: "schedule.activity",
        read_only: true,
        description: "Full context for one activity: dates, floats, predecessors/successors with link slack, the driving chain that sets its start, assignments and bound risks.",
        params: &[PROJ, BR, ("activity", Ty::Str, true, "Activity code or id")],
    },
    Tool {
        name: "get_float_paths",
        command: "schedule.critical_path",
        read_only: true,
        description: "Critical and near-critical float paths (most critical first).",
        params: &[
            PROJ,
            BR,
            ("k", Ty::Int, false, "Number of paths (default 3)"),
        ],
    },
    Tool {
        name: "schedule_health_check",
        command: "schedule.health",
        read_only: true,
        description: "DCMA 14-point assessment with offending activities.",
        params: &[PROJ, BR],
    },
    Tool {
        name: "add_activity",
        command: "activity.add",
        read_only: false,
        description: "Add an activity (deterministic or three-point duration), optionally FS-linked after predecessors.",
        params: &[
            PROJ,
            BR,
            ("code", Ty::Str, true, "Unique code"),
            ("name", Ty::Str, false, "Name"),
            ("duration", Ty::Est, false, "\"5d\" or {o,m,p,dist}"),
            ("wbs", Ty::Str, false, "WBS code"),
            (
                "kind",
                Ty::Str,
                false,
                "task | resource_dependent | level_of_effort | start_milestone | finish_milestone",
            ),
            (
                "constraint",
                Ty::Obj,
                false,
                "{kind: snet|snlt|fnet|fnlt|start_on|finish_on|mso|mfo|alap, date}",
            ),
            ("predecessors", Ty::Arr, false, "Activity codes"),
        ],
    },
    Tool {
        name: "update_activity",
        command: "activity.update",
        read_only: false,
        description: "Change name, duration/estimate, kind, WBS, calendar or constraint.",
        params: &[
            PROJ,
            BR,
            ("activity", Ty::Str, true, "Code or id"),
            ("name", Ty::Str, false, ""),
            ("duration", Ty::Est, false, "\"5d\" or {o,m,p,dist}"),
            ("kind", Ty::Str, false, ""),
            ("wbs", Ty::Str, false, ""),
            ("constraint", Ty::Obj, false, ""),
            ("clear_constraint", Ty::Bool, false, ""),
        ],
    },
    Tool {
        name: "delete_activity",
        command: "activity.delete",
        read_only: false,
        description: "Delete an activity (links, assignments, expenses and risk bindings cascade).",
        params: &[
            PROJ,
            BR,
            ("activity", Ty::Str, true, "Code or id"),
            (
                "reconnect",
                Ty::Bool,
                false,
                "Wire predecessors to successors",
            ),
        ],
    },
    Tool {
        name: "link_activities",
        command: "relationship.add",
        read_only: false,
        description: "Add a FS/SS/FF/SF relationship with optional lag (negative = lead). Loops are rejected.",
        params: &[
            PROJ,
            BR,
            ("pred", Ty::Str, true, "Predecessor code"),
            ("succ", Ty::Str, true, "Successor code"),
            ("kind", Ty::Str, false, "FS | SS | FF | SF"),
            ("lag", Ty::Dur, false, "e.g. \"2d\", \"-1d\""),
        ],
    },
    Tool {
        name: "unlink_activities",
        command: "relationship.delete",
        read_only: false,
        description: "Remove relationship(s) between two activities.",
        params: &[
            PROJ,
            BR,
            ("pred", Ty::Str, true, ""),
            ("succ", Ty::Str, true, ""),
            ("kind", Ty::Str, false, ""),
        ],
    },
    Tool {
        name: "update_progress",
        command: "activity.progress",
        read_only: false,
        description: "Record actual start/finish, percent complete or remaining duration.",
        params: &[
            PROJ,
            BR,
            ("activity", Ty::Str, true, ""),
            ("actual_start", Ty::Str, false, ""),
            ("actual_finish", Ty::Str, false, ""),
            ("pct", Ty::Num, false, "0..100"),
            ("remaining", Ty::Dur, false, ""),
        ],
    },
    Tool {
        name: "add_resource",
        command: "resource.add",
        read_only: false,
        description: "Add a labor/non-labor/material resource with max units per hour and rate.",
        params: &[
            PROJ,
            BR,
            ("code", Ty::Str, true, ""),
            ("name", Ty::Str, false, ""),
            ("kind", Ty::Str, false, "labor | non_labor | material"),
            (
                "max_units",
                Ty::Num,
                false,
                "Units per hour (1 = one person)",
            ),
            ("rate", Ty::Num, false, "Price per unit"),
        ],
    },
    Tool {
        name: "assign_resource",
        command: "resource.assign",
        read_only: false,
        description: "Assign a resource to an activity.",
        params: &[
            PROJ,
            BR,
            ("activity", Ty::Str, true, ""),
            ("resource", Ty::Str, true, ""),
            ("units", Ty::Num, false, "Budgeted units"),
            ("units_per_hour", Ty::Num, false, ""),
            ("curve", Ty::Str, false, "uniform | front | back | bell"),
        ],
    },
    Tool {
        name: "detect_overallocation",
        command: "resource.overallocation",
        read_only: true,
        description: "Days where demand exceeds capacity, per resource.",
        params: &[PROJ, BR],
    },
    Tool {
        name: "resource_histogram",
        command: "resource.histogram",
        read_only: true,
        description: "Remaining usage per resource per day/week (contention heatmap data).",
        params: &[
            PROJ,
            BR,
            ("period", Ty::Str, false, "day | week"),
            ("resource", Ty::Str, false, ""),
        ],
    },
    Tool {
        name: "auto_level_resources",
        command: "resource.level",
        read_only: false,
        description: "Serial resource leveling (priority: float, start, code). With apply=true, leveled starts are kept as SNET constraints.",
        params: &[
            PROJ,
            BR,
            ("apply", Ty::Bool, false, "Write leveled dates"),
            (
                "preserve_float",
                Ty::Bool,
                false,
                "Never delay beyond total float",
            ),
            ("resources", Ty::Arr, false, "Only these resource codes"),
        ],
    },
    Tool {
        name: "cost_summary",
        command: "cost.summary",
        read_only: true,
        description: "Budget rollup by WBS.",
        params: &[PROJ, BR],
    },
    Tool {
        name: "capture_baseline",
        command: "baseline.capture",
        read_only: false,
        description: "Freeze the current schedule and budget as a baseline.",
        params: &[PROJ, BR, ("name", Ty::Str, true, "")],
    },
    Tool {
        name: "compute_evm",
        command: "evm.report",
        read_only: true,
        description: "Earned value: PV, EV, AC, CPI, SPI, EAC, ETC, VAC, TCPI, earned schedule.",
        params: &[
            PROJ,
            BR,
            ("baseline", Ty::Str, false, ""),
            ("as_of", Ty::Str, false, ""),
            (
                "method",
                Ty::Str,
                false,
                "cpi | remaining_at_budget | cpi_spi",
            ),
        ],
    },
    Tool {
        name: "risk_register",
        command: "risk.list",
        read_only: true,
        description: "Risks with probabilities, bindings, expected delay and mitigations.",
        params: &[PROJ, BR],
    },
    Tool {
        name: "upsert_risk",
        command: "risk.add",
        read_only: false,
        description: "Add a risk bound to activities / WBS / resources / globally, with impacts [{delay|duration_factor|cost|cost_factor|degrade, once?}] and an optional post-mitigation state.",
        params: &[
            PROJ,
            BR,
            ("code", Ty::Str, true, ""),
            ("title", Ty::Str, false, ""),
            ("probability", Ty::Num, true, "0..1"),
            (
                "impacts",
                Ty::Arr,
                true,
                "e.g. [{\"delay\": {\"o\":\"2d\",\"m\":\"5d\",\"p\":\"12d\"}}, {\"cost\": 25000}]",
            ),
            (
                "bindings",
                Ty::Arr,
                false,
                "Activity codes, {\"wbs\": code}, {\"resource\": code} or \"global\"",
            ),
            ("post", Ty::Obj, false, "{probability, impacts?}"),
            ("proximity", Ty::Obj, false, "{from, to}"),
        ],
    },
    Tool {
        name: "inject_risk_event",
        command: "risk.inject",
        read_only: false,
        description: "Create a risk (same arguments as upsert_risk) and optionally realize it at once — the quick what-if in a sandbox.",
        params: &[
            PROJ,
            BR,
            ("code", Ty::Str, true, ""),
            ("title", Ty::Str, false, ""),
            ("probability", Ty::Num, true, ""),
            ("impacts", Ty::Arr, true, ""),
            ("bindings", Ty::Arr, false, ""),
            ("realize", Ty::Bool, false, ""),
            ("choice", Ty::Str, false, "most_likely | pessimistic"),
        ],
    },
    Tool {
        name: "realize_risk",
        command: "risk.realize",
        read_only: false,
        description: "Turn a potential risk into graph edits (delays, costs, contingent branch), trigger secondary risks and report the schedule delta.",
        params: &[
            PROJ,
            BR,
            ("risk", Ty::Str, true, "Risk code"),
            ("choice", Ty::Str, false, "most_likely | pessimistic"),
            ("apply", Ty::Bool, false, "false = preview only"),
        ],
    },
    Tool {
        name: "link_secondary_risk",
        command: "risk.trigger",
        read_only: false,
        description: "If `risk` occurs, `target` occurs with at least this probability.",
        params: &[
            PROJ,
            BR,
            ("risk", Ty::Str, true, ""),
            ("target", Ty::Str, true, ""),
            ("probability", Ty::Num, true, ""),
        ],
    },
    Tool {
        name: "set_correlation",
        command: "risk.correlate",
        read_only: false,
        description: "Correlate uncertain durations (activity codes) and risk occurrences (\"risk:CODE\") with one rank correlation.",
        params: &[
            PROJ,
            BR,
            ("members", Ty::Arr, true, ""),
            ("rho", Ty::Num, true, "-1..1"),
        ],
    },
    Tool {
        name: "generate_mitigation_subgraph",
        command: "mitigation.propose",
        read_only: true,
        description: "Deterministic mitigation proposal (not applied): crash, fast_track, add_resource, contingency_buffer or alternative_path.",
        params: &[
            PROJ,
            BR,
            ("risk", Ty::Str, true, ""),
            ("strategy", Ty::Str, true, ""),
            ("pct", Ty::Num, false, "Reduction/overlap/share %"),
            ("cost_per_day", Ty::Num, false, ""),
            ("duration", Ty::Dur, false, "Buffer / alternative duration"),
            ("before", Ty::Str, false, "Activity protected by a buffer"),
            ("cost", Ty::Num, false, ""),
        ],
    },
    Tool {
        name: "attach_mitigation",
        command: "mitigation.attach",
        read_only: false,
        description: "Accept a mitigation (same arguments as generate_mitigation_subgraph); optional post-mitigation probability.",
        params: &[
            PROJ,
            BR,
            ("risk", Ty::Str, true, ""),
            ("strategy", Ty::Str, true, ""),
            ("pct", Ty::Num, false, ""),
            ("cost_per_day", Ty::Num, false, ""),
            ("duration", Ty::Dur, false, ""),
            ("before", Ty::Str, false, ""),
            ("cost", Ty::Num, false, ""),
            ("post_probability", Ty::Num, false, ""),
        ],
    },
    Tool {
        name: "run_impact_simulation",
        command: "simulation.run",
        read_only: true,
        description: "Monte Carlo: P10–P90 finish and cost, probability of meeting targets, criticality, sensitivity, risk tornado. With compare_to, a paired (common random numbers) delta vs that branch.",
        params: &[
            PROJ,
            BR,
            ("iterations", Ty::Int, false, "100..200000 (default 2000)"),
            ("seed", Ty::Int, false, ""),
            ("mode", Ty::Str, false, "pre | post mitigation"),
            ("finish_by", Ty::Str, false, ""),
            ("budget", Ty::Num, false, ""),
            (
                "compare_to",
                Ty::Str,
                false,
                "Branch to compare against, e.g. live",
            ),
        ],
    },
    Tool {
        name: "get_simulation_result",
        command: "simulation.get",
        read_only: true,
        description: "Stored simulation details.",
        params: &[
            ("run", Ty::Str, true, ""),
            (
                "include",
                Ty::Arr,
                false,
                "histogram, criticality, sensitivity, convergence, samples",
            ),
        ],
    },
    Tool {
        name: "create_sandbox",
        command: "sandbox.create",
        read_only: true,
        description: "Fork a what-if sandbox (instant). Agents make all changes there.",
        params: &[
            PROJ,
            ("name", Ty::Str, true, ""),
            ("from", Ty::Str, false, "Source branch (default live)"),
            ("ttl_minutes", Ty::Int, false, "Auto-expire"),
        ],
    },
    Tool {
        name: "list_sandboxes",
        command: "sandbox.list",
        read_only: true,
        description: "Branches with status and versions.",
        params: &[PROJ],
    },
    Tool {
        name: "diff_branches",
        command: "sandbox.diff",
        read_only: true,
        description: "Entity changes and schedule delta of a sandbox vs live (or another base).",
        params: &[
            PROJ,
            ("head", Ty::Str, true, "Sandbox"),
            ("base", Ty::Str, false, "Default live"),
        ],
    },
    Tool {
        name: "commit_sandbox_to_baseline",
        command: "sandbox.commit",
        read_only: false,
        description: "Three-way merge a sandbox into live (needs human approval under the default policy), optionally capturing a baseline.",
        params: &[
            PROJ,
            ("sandbox", Ty::Str, true, ""),
            (
                "strategy",
                Ty::Str,
                false,
                "reject_on_conflict | prefer_sandbox | prefer_live",
            ),
            ("capture_baseline", Ty::Str, false, "Baseline name"),
        ],
    },
    Tool {
        name: "discard_sandbox",
        command: "sandbox.discard",
        read_only: true,
        description: "Throw a sandbox away.",
        params: &[PROJ, ("sandbox", Ty::Str, true, "")],
    },
    Tool {
        name: "undo",
        command: "edit.undo",
        read_only: false,
        description: "Undo the last change on a branch.",
        params: &[PROJ, BR],
    },
    Tool {
        name: "redo",
        command: "edit.redo",
        read_only: false,
        description: "Redo.",
        params: &[PROJ, BR],
    },
];

/// JSON Schema (`inputSchema`) for a parameter list.
pub fn schema(params: &[Param]) -> Value {
    let mut props = Map::new();
    let mut required = Vec::new();
    for (name, ty, req, desc) in params {
        let mut p = match ty {
            Ty::Str | Ty::Dur => json!({ "type": "string" }),
            Ty::Num => json!({ "type": "number" }),
            Ty::Int => json!({ "type": "integer", "minimum": 0 }),
            Ty::Bool => json!({ "type": "boolean" }),
            Ty::Est => json!({ "oneOf": [{ "type": "string" }, { "type": "object", "properties": {
                "o": { "type": "string" }, "m": { "type": "string" }, "p": { "type": "string" },
                "dist": { "type": "string", "enum": ["triangular", "pert", "uniform"] } }, "required": ["o", "p"] }] }),
            Ty::Arr => json!({ "type": "array" }),
            Ty::Obj => json!({ "type": "object" }),
        };
        if !desc.is_empty() {
            p["description"] = json!(desc);
        }
        props.insert((*name).to_owned(), p);
        if *req {
            required.push(*name);
        }
    }
    json!({ "type": "object", "properties": props, "required": required, "additionalProperties": false })
}

/// Arguments must match the schema's required keys and primitive types.
pub fn check(params: &[Param], args: &Value) -> Result<(), String> {
    let Value::Object(m) = args else {
        return Err("arguments must be an object".into());
    };
    for (name, ty, req, _) in params {
        match m.get(*name) {
            None | Some(Value::Null) if *req => {
                return Err(format!("missing required argument `{name}`"));
            }
            None | Some(Value::Null) => {}
            Some(v) => {
                let ok = match ty {
                    Ty::Str | Ty::Dur => v.is_string(),
                    Ty::Num => v.is_number() || v.is_string(),
                    Ty::Int => v.is_u64(),
                    Ty::Bool => v.is_boolean(),
                    Ty::Est => v.is_string() || v.is_object() || v.is_number(),
                    Ty::Arr => v.is_array(),
                    Ty::Obj => v.is_object(),
                };
                if !ok {
                    return Err(format!("argument `{name}` has the wrong type"));
                }
            }
        }
    }
    if let Some(k) = m.keys().find(|k| !params.iter().any(|p| p.0 == k.as_str())) {
        return Err(format!("unknown argument `{k}`"));
    }
    Ok(())
}
