//! Workspace tasks (FND-03), after GridCraft's `cargo xtask`:
//!
//! * `layers`  — enforce the crate layering of CLAUDE.md / ADR-0007.
//! * `parity`  — regenerate `docs/parity-checklist.md` from the command catalog
//!   and the P6 feature list below.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// (crate, layer). A crate may depend only on crates of the same or a lower layer.
const LAYERS: &[(&str, u8)] = &[
    ("prswt-core", 0),
    ("prswt-calendar", 0),
    ("prswt-cpm", 1),
    ("prswt-resource", 1),
    ("prswt-evm", 1),
    ("prswt-risk", 1),
    ("prswt-state", 2),
    ("prswt-store", 3),
    ("prswt-app", 3),
    ("prswt-formats", 3),
    ("prswt-mcp", 4),
    ("prswt-api", 4),
    ("prswt-ui", 4),
    ("prswt-cli", 5),
    ("prswt-desktop", 5),
    ("prswt-server", 5),
];

/// External crates that must never appear below layer 3 (IO, async, GUI).
const IO_CRATES: &[&str] = &[
    "tokio", "sqlx", "reqwest", "axum", "hyper", "egui", "eframe", "winit", "rfd",
];

fn main() -> ExitCode {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let task = std::env::args().nth(1).unwrap_or_default();
    let result = match task.as_str() {
        "layers" => layers(&root),
        "parity" => parity(&root.join("..")),
        _ => Err("usage: cargo xtask <layers|parity>".to_owned()),
    };
    match result {
        Ok(msg) => {
            println!("{msg}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

fn layers(root: &Path) -> Result<String, String> {
    let layer: BTreeMap<&str, u8> = LAYERS.iter().copied().collect();
    let mut problems = Vec::new();
    let mut checked = 0;
    let crates = std::fs::read_dir(root.join("crates")).map_err(|e| e.to_string())?;
    for entry in crates.flatten() {
        let manifest = entry.path().join("Cargo.toml");
        let Ok(text) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(&own) = layer.get(name.as_str()) else {
            problems.push(format!("{name}: not assigned a layer in xtask LAYERS"));
            continue;
        };
        checked += 1;
        for dep in normal_dependencies(&text) {
            if let Some(&l) = layer.get(dep.as_str())
                && l > own
            {
                problems.push(format!("{name} (L{own}) depends on {dep} (L{l})"));
            }
            if own < 3 && IO_CRATES.contains(&dep.as_str()) {
                problems.push(format!("{name} (L{own}) must not depend on IO crate {dep}"));
            }
        }
    }
    if problems.is_empty() {
        Ok(format!("layers ok ({checked} crates)"))
    } else {
        Err(problems.join("\n"))
    }
}

/// Dependency names from `[dependencies]` (not dev/build dependencies).
fn normal_dependencies(toml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in toml.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            inside = l == "[dependencies]";
            continue;
        }
        if inside && let Some((k, _)) = l.split_once('=') {
            let k = k.trim().trim_end_matches(".workspace").trim();
            if !k.is_empty() && !k.starts_with('#') {
                out.push(k.to_owned());
            }
        }
    }
    out
}

/// P6 feature areas and whether PRSWT covers them (assessed, with pointers).
const P6_FEATURES: &[(&str, &str, bool, &str)] = &[
    (
        "Scheduling",
        "Forward/backward pass, FS/SS/FF/SF, leads/lags",
        true,
        "prswt-cpm passes.rs",
    ),
    (
        "Scheduling",
        "Lag calendar options (pred/succ/project/24h)",
        true,
        "LagCalendar",
    ),
    (
        "Scheduling",
        "Constraints: SNET/SNLT/FNET/FNLT/On/Mandatory/ALAP",
        true,
        "passes.rs",
    ),
    (
        "Scheduling",
        "Must finish by, negative float",
        true,
        "late_end",
    ),
    (
        "Scheduling",
        "Retained logic / progress override",
        true,
        "ProgressMode",
    ),
    (
        "Scheduling",
        "Total float start/finish/smallest, free float",
        true,
        "floats()",
    ),
    (
        "Scheduling",
        "Critical: TF threshold / longest path",
        true,
        "analysis.rs",
    ),
    ("Scheduling", "Multiple float paths", true, "float_paths"),
    (
        "Scheduling",
        "Activity types: task, resource-dependent, LOE, milestones, WBS summary",
        true,
        "ActivityKind",
    ),
    (
        "Scheduling",
        "Out-of-sequence progress detection report",
        false,
        "gaps.md",
    ),
    (
        "Scheduling",
        "Expected finish dates / suspend-resume",
        false,
        "gaps.md",
    ),
    (
        "Calendars",
        "Global/project/resource calendars, exceptions, inheritance",
        true,
        "prswt-calendar",
    ),
    (
        "Calendars",
        "Shifts crossing midnight",
        true,
        "interval merge",
    ),
    ("WBS/OBS", "WBS hierarchy with rollups", true, "wbs_rollup"),
    (
        "WBS/OBS",
        "OBS / responsible manager security",
        false,
        "gaps.md",
    ),
    (
        "Resources",
        "Labor / non-labor / material, max units, rates (5 types)",
        true,
        "prswt-resource",
    ),
    ("Resources", "Resource curves", true, "SpreadCurve"),
    (
        "Resources",
        "Usage profiles, overallocation",
        true,
        "histogram",
    ),
    (
        "Resources",
        "Resource leveling (priority, preserve float)",
        true,
        "level.rs",
    ),
    (
        "Resources",
        "Roles and role-based assignment",
        false,
        "gaps.md",
    ),
    (
        "Costs",
        "Expenses, cost accounts, WBS budgets",
        true,
        "cost_rollup",
    ),
    (
        "EVM",
        "Baselines, PV/EV/AC, CPI/SPI, EAC/ETC/VAC/TCPI",
        true,
        "prswt-evm",
    ),
    ("EVM", "Earned schedule SPI(t)", true, "evaluate"),
    (
        "EVM",
        "Multiple baselines compare in Gantt",
        false,
        "gaps.md",
    ),
    ("Analysis", "Schedule check (DCMA 14)", true, "dcma.rs"),
    (
        "Risk",
        "Risk register with pre/post mitigation",
        true,
        "Risk",
    ),
    (
        "Risk",
        "Monte Carlo (P6 needs Primavera Risk Analysis)",
        true,
        "prswt-risk",
    ),
    (
        "Risk",
        "Correlation, risk tornado, criticality, P-values",
        true,
        "sim.rs",
    ),
    (
        "What-if",
        "Sandboxes with diff/merge (P6: copy project)",
        true,
        "prswt-state",
    ),
    (
        "Collaboration",
        "Undo/redo, audit log",
        true,
        "Repo::undo, events",
    ),
    ("Interop", "XER import/export", true, "prswt-formats"),
    (
        "Interop",
        "MS Project XML (MSPDI)",
        true,
        "prswt-formats mspdi.rs",
    ),
    (
        "Agents",
        "MCP server (tools/resources/prompts)",
        true,
        "prswt-mcp",
    ),
    ("UI", "Gantt chart, activity table", true, "prswt-ui"),
    ("UI", "Network (PERT) diagram", false, "gaps.md"),
];

fn parity(repo_root: &Path) -> Result<String, String> {
    let mut md = String::new();
    let done = P6_FEATURES.iter().filter(|f| f.2).count();
    let _ = writeln!(
        md,
        "# Parity checklist (generated by `cargo xtask parity`)\n"
    );
    let _ = writeln!(
        md,
        "Target: Oracle Primavera P6 EPPM (feature names only; clean-room).\n"
    );
    let _ = writeln!(
        md,
        "**Feature areas:** {done} / {} covered.\n",
        P6_FEATURES.len()
    );
    let _ = writeln!(md, "| Area | Feature | Status | Where |\n|---|---|---|---|");
    for (area, feat, ok, where_) in P6_FEATURES {
        let _ = writeln!(
            md,
            "| {area} | {feat} | {} | `{where_}` |",
            if *ok { "✅" } else { "⬜" }
        );
    }
    let cat = prswt_app::catalog();
    let _ = writeln!(md, "\n## Command catalog ({} commands)\n", cat.len());
    let _ = writeln!(md, "| Id | Label | P6 menu | Writes |\n|---|---|---|---|");
    for c in cat {
        let _ = writeln!(
            md,
            "| `{}` | {} | {} | {} |",
            c.id,
            c.label,
            c.menu,
            if c.writes { "yes" } else { "no" }
        );
    }
    let path: PathBuf = repo_root.join("docs").join("parity-checklist.md");
    std::fs::create_dir_all(path.parent().unwrap_or(repo_root)).map_err(|e| e.to_string())?;
    std::fs::write(&path, md).map_err(|e| e.to_string())?;
    Ok(format!(
        "wrote {} ({done}/{} areas, {} commands)",
        path.display(),
        P6_FEATURES.len(),
        cat.len()
    ))
}
