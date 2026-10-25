//! MCP-10: agent workflow prompts (`04-MCP-Spec.md` §3).

use serde_json::{Value, json};

struct Prompt {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    args: &'static [(&'static str, &'static str, bool)],
    body: &'static str,
}

const PROMPTS: &[Prompt] = &[
    Prompt {
        name: "risk_auditor",
        title: "Autonomous risk auditor",
        description: "Scan the schedule for hidden fragility and rank mitigations by P80 gain per cost.",
        args: &[("project", "Project name", false)],
        body: "You are a schedule risk auditor for project {project}. Work only in sandboxes; never ask to commit without showing evidence.\n\
1. Call schedule_health_check and get_float_paths (k=5). Note DCMA failures and paths with < 5d float.\n\
2. Call run_impact_simulation (iterations 3000). Record P50/P80 finish, prob_finish_by and the top risk_tornado and top_sensitivity entries.\n\
3. Fragility = activities with criticality > 0.3 that are not on the deterministic critical path, and the top 5 tornado drivers.\n\
4. For each of the top 3 drivers: create_sandbox (name it after the driver), generate_mitigation_subgraph with a suitable strategy, attach_mitigation, \
then run_impact_simulation with compare_to=\"live\" and the same seed.\n\
5. Report a ranked table: driver, strategy, ΔP80 (days, paired), cost, ΔP80 per 10k. Recommend at most two to commit and say which sandbox holds each.",
    },
    Prompt {
        name: "supply_chain_correlator",
        title: "Supply-chain correlator",
        description: "Translate an external delay (supplier, port, weather) into graph changes and quantify it.",
        args: &[
            ("project", "Project name", false),
            ("event", "Description of the external event", true),
        ],
        body: "External event for project {project}: \"{event}\".\n\
1. Identify affected work: query_activities with text filters, explain_activity_dates for candidates, resource_histogram if a resource is involved.\n\
2. create_sandbox named \"supply-<short>\". In it, inject_risk_event with a calibrated three-point delay (state your o/m/p reasoning) bound to the affected activities or resource; realize=false.\n\
3. run_impact_simulation with compare_to=\"live\": report Δ P50/P80 finish and cost.\n\
4. If ΔP80 > 5 days, propose and test one mitigation in a second sandbox.\n\
5. Summarise for a human with diff_branches output; do not commit.",
    },
    Prompt {
        name: "executive_briefing",
        title: "Executive briefing",
        description: "One-page status brief: dates, cost, risk, decisions.",
        args: &[
            ("project", "Project name", false),
            ("audience", "board | pmo", false),
        ],
        body: "Write a one-page briefing on project {project} for the {audience}.\n\
Gather: get_schedule_summary, compute_evm (if a baseline exists), run_impact_simulation (2000 iterations), risk_register.\n\
Structure: RAG status with one-line reason; deterministic vs P50/P80 finish against the target; cost forecast (EAC, CPI); top 3 risks with owner-ready actions; \
decisions needed. Use plain numbers and dates, no jargon beyond P50/P80, SPI, CPI.",
    },
    Prompt {
        name: "mitigation_optimizer",
        title: "Mitigation optimizer",
        description: "Compare strategies for one risk in separate sandboxes and pick the Pareto set.",
        args: &[
            ("project", "Project name", false),
            ("risk", "Risk code", true),
            ("budget", "Maximum mitigation spend", false),
        ],
        body: "Optimise mitigation of risk {risk} in project {project} (budget {budget}).\n\
For each strategy in [crash, fast_track, add_resource, contingency_buffer, alternative_path]: create_sandbox \"opt-<strategy>\", \
generate_mitigation_subgraph, attach_mitigation, run_impact_simulation compare_to=\"live\" with one fixed seed.\n\
Discard sandboxes whose cost exceeds the budget. Return the Pareto set (ΔP80 vs cost) and keep only those sandboxes open (discard_sandbox the rest).",
    },
    Prompt {
        name: "schedule_repair",
        title: "Schedule repair",
        description: "Fix DCMA findings (missing logic, hard constraints, leads) in a sandbox.",
        args: &[("project", "Project name", false)],
        body: "Repair schedule quality issues in project {project}.\n\
1. schedule_health_check. 2. create_sandbox \"repair\". 3. For missing logic add the most plausible links (explain each); \
replace hard constraints with soft ones; turn leads into positive-lag alternatives. 4. Re-run schedule_health_check in the sandbox and diff_branches. \
5. Present before/after scores and the change list; a human commits.",
    },
];

pub fn list() -> Vec<Value> {
    PROMPTS
        .iter()
        .map(|p| json!({
            "name": p.name,
            "title": p.title,
            "description": p.description,
            "arguments": p.args.iter().map(|(n, d, r)| json!({ "name": n, "description": d, "required": r })).collect::<Vec<_>>(),
        }))
        .collect()
}

pub fn get(name: &str, args: &Value) -> Option<Value> {
    let p = PROMPTS.iter().find(|p| p.name == name)?;
    let mut text = p.body.to_owned();
    for (arg, _, _) in p.args {
        let v = args
            .get(*arg)
            .and_then(Value::as_str)
            .unwrap_or(match *arg {
                "project" => "(the open project)",
                "audience" => "board",
                "budget" => "unlimited",
                _ => "(unspecified)",
            });
        text = text.replace(
            &format!("{{{arg}}}"),
            &v.chars().take(500).collect::<String>(),
        );
    }
    Some(json!({
        "description": p.description,
        "messages": [{ "role": "user", "content": { "type": "text", "text": text } }],
    }))
}
