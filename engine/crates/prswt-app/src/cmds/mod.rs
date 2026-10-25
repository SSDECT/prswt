//! The command catalog (APP-02). Each entry's id follows P6's menu/action
//! naming; `menu` records where P6 users would look for it.

mod analysis;
mod network;
mod project;
mod risk;
mod sandbox;
mod sync;

use crate::CommandSpec;

macro_rules! spec {
    ($id:literal, $label:literal, $menu:literal, $writes:literal, $run:path, $params:literal) => {
        CommandSpec {
            id: $id,
            label: $label,
            menu: $menu,
            params: $params,
            writes: $writes,
            run: $run,
        }
    };
}

static CATALOG: &[CommandSpec] = &[
    // cloud sync (SYNC-01..03, ADR-0010)
    spec!(
        "sync.link",
        "Link to cloud project",
        "File ▸ Cloud",
        true,
        sync::link,
        "project?, server (https origin)"
    ),
    spec!(
        "sync.status",
        "Cloud sync status",
        "File ▸ Cloud",
        false,
        sync::status,
        "project?"
    ),
    spec!(
        "sync.outbox",
        "Unsent cloud changes",
        "File ▸ Cloud",
        false,
        sync::outbox,
        "project?"
    ),
    spec!(
        "sync.events",
        "Cloud events since",
        "File ▸ Cloud",
        false,
        sync::events,
        "project?, since? ({branch_id: version})"
    ),
    spec!(
        "sync.receive",
        "Receive branch changes",
        "File ▸ Cloud",
        true,
        sync::receive,
        "project?, meta (branch metadata), base_version, commands, base? (new sandbox)"
    ),
    spec!(
        "sync.apply",
        "Apply cloud changes",
        "File ▸ Cloud",
        true,
        sync::apply,
        "project?, branches: [{meta, base?, events}]"
    ),
    spec!(
        "sandbox.propose",
        "Propose sandbox for review",
        "Project ▸ What-if ▸ Propose",
        true,
        sync::propose,
        "project?, sandbox, note?, withdraw?"
    ),
    // project & editing
    spec!(
        "project.create",
        "New project",
        "File ▸ New",
        true,
        project::create,
        "name, start (YYYY-MM-DD[THH:MM]), calendar? (5x8|6x10|7x24), data_date?"
    ),
    spec!(
        "project.list",
        "Open projects",
        "Window ▸ Projects",
        false,
        project::list,
        "brief? (true = skip scheduling; no `finish`)"
    ),
    spec!(
        "project.update",
        "Project details",
        "Project ▸ Details",
        true,
        project::update,
        "project?, branch?, name?, data_date?, planned_start?, must_finish_by?, clear_must_finish_by?, critical_float?, near_critical_float?, lag_calendar?, progress_mode?"
    ),
    spec!(
        "sample.load",
        "Open sample project",
        "File ▸ Samples",
        true,
        project::sample,
        "name (bridge|turnaround)"
    ),
    spec!(
        "commands.list",
        "List commands",
        "Help ▸ Commands",
        false,
        project::commands,
        "search?"
    ),
    spec!(
        "commands.apply",
        "Apply raw command batch",
        "Tools ▸ Batch",
        true,
        project::apply_raw,
        "project?, branch?, commands: [state commands], expected_version?"
    ),
    spec!(
        "project.lookups",
        "Pick lists",
        "View ▸ Lookups",
        false,
        project::lookups,
        "project?, branch?"
    ),
    spec!(
        "project.close",
        "Close project",
        "File ▸ Close",
        true,
        project::close,
        "project?"
    ),
    spec!(
        "file.open_project",
        "Open project file",
        "File ▸ Open",
        true,
        project::open_project,
        "text (.prswt project file content)"
    ),
    spec!(
        "file.save_project",
        "Save project file",
        "File ▸ Save",
        false,
        project::save_project,
        "project?"
    ),
    spec!(
        "file.import_xer",
        "Import XER",
        "File ▸ Import ▸ Primavera XER",
        true,
        project::import_xer,
        "text (XER file content; core tables)"
    ),
    spec!(
        "file.export_xer",
        "Export XER",
        "File ▸ Export ▸ Primavera XER",
        false,
        project::export_xer,
        "project?, branch?"
    ),
    spec!(
        "file.import_mspdi",
        "Import MS Project XML",
        "File ▸ Import ▸ Microsoft Project XML",
        true,
        project::import_mspdi,
        "text (MSPDI XML content)"
    ),
    spec!(
        "file.export_mspdi",
        "Export MS Project XML",
        "File ▸ Export ▸ Microsoft Project XML",
        false,
        project::export_mspdi,
        "project?, branch?"
    ),
    spec!(
        "edit.undo",
        "Undo",
        "Edit ▸ Undo",
        true,
        project::undo,
        "project?, branch?"
    ),
    spec!(
        "edit.redo",
        "Redo",
        "Edit ▸ Redo",
        true,
        project::redo,
        "project?, branch?"
    ),
    spec!(
        "project.history",
        "Event log",
        "Project ▸ History",
        false,
        project::history,
        "project?, branch?, since?"
    ),
    // network
    spec!(
        "calendar.add",
        "New calendar",
        "Enterprise ▸ Calendars",
        true,
        network::calendar_add,
        "name, week? (5x8|6x10|7x24), base?, hours_per_day?"
    ),
    spec!(
        "calendar.exception",
        "Calendar exception",
        "Enterprise ▸ Calendars ▸ Exceptions",
        true,
        network::calendar_exception,
        "calendar?, date, work? ([[\"08:00\",\"12:00\"],...]; empty = non-working)"
    ),
    spec!(
        "wbs.add",
        "Add WBS element",
        "Project ▸ WBS ▸ Add",
        true,
        network::wbs_add,
        "code, name, parent? (WBS code)"
    ),
    spec!(
        "activity.add",
        "Add activity",
        "Project ▸ Activities ▸ Add",
        true,
        network::activity_add,
        "code, name?, duration (\"5d\" or {o,m,p,dist?}), wbs?, kind?, calendar?, constraint? {kind,date}, predecessors? [codes]"
    ),
    spec!(
        "activity.update",
        "Edit activity",
        "Project ▸ Activities ▸ Details",
        true,
        network::activity_update,
        "activity, name?, duration?, kind?, wbs?, calendar?, constraint?, clear_constraint?"
    ),
    spec!(
        "activity.delete",
        "Delete activity",
        "Project ▸ Activities ▸ Delete",
        true,
        network::activity_delete,
        "activity, reconnect?"
    ),
    spec!(
        "activity.progress",
        "Update progress",
        "Project ▸ Activities ▸ Status",
        true,
        network::activity_progress,
        "activity, actual_start?, actual_finish?, pct?, remaining?, actual_cost? (cost to date, split over its resources and expenses by planned cost)"
    ),
    spec!(
        "relationship.add",
        "Assign relationship",
        "Project ▸ Activities ▸ Relationships",
        true,
        network::relationship_add,
        "pred, succ, kind? (FS|SS|FF|SF), lag?"
    ),
    spec!(
        "relationship.delete",
        "Remove relationship",
        "Project ▸ Activities ▸ Relationships",
        true,
        network::relationship_delete,
        "pred, succ, kind?"
    ),
    spec!(
        "resource.add",
        "Add resource",
        "Enterprise ▸ Resources",
        true,
        network::resource_add,
        "code, name?, kind? (labor|non_labor|material), max_units? (units/h), rate?, calendar?"
    ),
    spec!(
        "resource.assign",
        "Assign resource",
        "Project ▸ Activities ▸ Resources",
        true,
        network::resource_assign,
        "activity, resource, units? (default: duration hours x units_per_hour), units_per_hour?, curve?"
    ),
    spec!(
        "expense.add",
        "Add expense",
        "Project ▸ Expenses",
        true,
        network::expense_add,
        "activity, name, amount, accrual? (start|end|uniform)"
    ),
    // analysis
    spec!(
        "schedule.run",
        "Schedule (F9)",
        "Tools ▸ Schedule",
        false,
        analysis::run,
        "project?, branch?"
    ),
    spec!(
        "schedule.activities",
        "Activity table",
        "Project ▸ Activities",
        false,
        analysis::activities,
        "project?, branch?, critical?, max_float?, text?, wbs?, sort? (float|start|code), limit?, offset?"
    ),
    spec!(
        "schedule.activity",
        "Activity details",
        "Project ▸ Activities ▸ Details",
        false,
        analysis::activity,
        "activity, project?, branch?"
    ),
    spec!(
        "schedule.critical_path",
        "Critical / float paths",
        "Tools ▸ Schedule ▸ Float paths",
        false,
        analysis::paths,
        "project?, branch?, k?"
    ),
    spec!(
        "schedule.health",
        "Schedule check (DCMA 14)",
        "Tools ▸ Check Schedule",
        false,
        analysis::health,
        "project?, branch?"
    ),
    spec!(
        "resource.histogram",
        "Resource usage",
        "Project ▸ Resource Usage Profile",
        false,
        analysis::histogram,
        "project?, branch?, period? (day|week), resource?"
    ),
    spec!(
        "resource.overallocation",
        "Overallocation",
        "Project ▸ Resource Usage Profile",
        false,
        analysis::overallocation,
        "project?, branch?"
    ),
    spec!(
        "resource.level",
        "Level resources",
        "Tools ▸ Level Resources",
        true,
        analysis::level,
        "project?, branch?, apply?, preserve_float?, resources?"
    ),
    spec!(
        "cost.summary",
        "Cost rollup",
        "Project ▸ WBS ▸ Budget",
        false,
        analysis::cost,
        "project?, branch?"
    ),
    spec!(
        "baseline.capture",
        "Maintain baselines",
        "Project ▸ Maintain Baselines",
        true,
        analysis::baseline,
        "name, project?, branch?"
    ),
    spec!(
        "evm.report",
        "Earned value",
        "Reports ▸ Earned Value",
        false,
        analysis::evm,
        "baseline?, as_of?, method? (cpi|remaining_at_budget|cpi_spi)"
    ),
    // risk
    spec!(
        "risk.add",
        "Add risk",
        "Project ▸ Risks ▸ Add",
        true,
        risk::add,
        "code, title?, probability, impacts [{delay|duration_factor|cost|cost_factor|degrade, once?}], bindings [code | {wbs} | {resource} | \"global\"], post?, proximity?"
    ),
    spec!(
        "risk.update",
        "Edit risk",
        "Project ▸ Risks ▸ Details",
        true,
        risk::update,
        "risk, probability?, status?, post_probability?"
    ),
    spec!(
        "risk.list",
        "Risk register",
        "Project ▸ Risks",
        false,
        risk::list,
        "project?, branch?"
    ),
    spec!(
        "risk.inject",
        "Inject risk event",
        "Tools ▸ What-if ▸ Inject",
        true,
        risk::inject,
        "as risk.add, plus realize? (default false)"
    ),
    spec!(
        "risk.realize",
        "Realize risk",
        "Project ▸ Risks ▸ Realize",
        true,
        risk::realize,
        "risk, choice? (most_likely|pessimistic), apply? (default true)"
    ),
    spec!(
        "risk.trigger",
        "Link secondary risk",
        "Project ▸ Risks ▸ Triggers",
        true,
        risk::trigger,
        "risk, target, probability"
    ),
    spec!(
        "risk.correlate",
        "Correlate uncertainties",
        "Project ▸ Risks ▸ Correlation",
        true,
        risk::correlate,
        "members [activity code | \"risk:CODE\"], rho"
    ),
    spec!(
        "mitigation.propose",
        "Propose mitigation",
        "Tools ▸ Mitigation ▸ Propose",
        false,
        risk::propose,
        "risk, strategy (crash|fast_track|add_resource|contingency_buffer|alternative_path), pct?, cost_per_day?, duration?, before?, cost?"
    ),
    spec!(
        "mitigation.attach",
        "Accept mitigation",
        "Tools ▸ Mitigation ▸ Accept",
        true,
        risk::attach,
        "risk, strategy params as in mitigation.propose, post_probability?, cost? (what the plan costs; default the strategy's own cost)"
    ),
    spec!(
        "simulation.run",
        "Run risk analysis",
        "Tools ▸ Risk Analysis ▸ Run",
        false,
        risk::simulate,
        "project?, branch?, iterations?, seed?, mode? (pre|post), finish_by?, budget?, compare_to?"
    ),
    spec!(
        "simulation.get",
        "Risk analysis results",
        "Tools ▸ Risk Analysis ▸ Results",
        false,
        risk::simulation_get,
        "run, include? [histogram, criticality, sensitivity, samples]"
    ),
    // sandboxes
    spec!(
        "sandbox.create",
        "Create what-if sandbox",
        "Tools ▸ What-if ▸ New",
        true,
        sandbox::create,
        "name, from? (branch), ttl_minutes?"
    ),
    spec!(
        "sandbox.list",
        "Sandboxes",
        "Tools ▸ What-if",
        false,
        sandbox::list,
        "project?"
    ),
    spec!(
        "sandbox.diff",
        "Compare branches",
        "Tools ▸ What-if ▸ Compare",
        false,
        sandbox::diff,
        "head (sandbox), base? (default live)"
    ),
    spec!(
        "sandbox.commit",
        "Commit sandbox to live",
        "Tools ▸ What-if ▸ Commit",
        true,
        sandbox::commit,
        "sandbox, strategy? (reject_on_conflict|prefer_sandbox|prefer_live), capture_baseline?"
    ),
    spec!(
        "sandbox.discard",
        "Discard sandbox",
        "Tools ▸ What-if ▸ Discard",
        true,
        sandbox::discard,
        "sandbox"
    ),
];

/// Every registered command, in catalog order (also used by `xtask parity`).
pub fn catalog() -> &'static [CommandSpec] {
    CATALOG
}
