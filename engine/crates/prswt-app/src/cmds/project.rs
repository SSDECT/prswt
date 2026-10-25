use std::collections::BTreeMap;

use prswt_core::ids::{CalendarId, ProjectId, WbsId};
use prswt_core::model::{
    CalendarDef, DayInterval, LagCalendar, ProgressMode, Project, ScheduleSettings, WbsNode,
    WorkWeek,
};
use prswt_core::{EngineError, ErrorCode, WorkMinutes};
use prswt_state::{Command, MAX_FILE_BYTES, ProjectFile, ProjectState, Repo};
use serde_json::{Value, json};

use crate::params::P;
use crate::{Ctx, Workspace, catalog, samples};

pub fn create(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let name = p.str("name")?.to_owned();
    let start = p.instant("start")?;
    let data_date = p.opt_instant("data_date")?.unwrap_or(start);
    let (week, mpd) = week_preset(p.opt_str("calendar")?.unwrap_or("5x8"))?;
    let state = new_state(&name, start, data_date, week, mpd);
    let pid = open(w, ctx, state)?;
    Ok(json!({ "project": pid, "name": name, "branch": "live" }))
}

pub(crate) fn new_state(
    name: &str,
    start: prswt_core::Instant,
    data_date: prswt_core::Instant,
    week: WorkWeek,
    mpd: i64,
) -> ProjectState {
    let cal = CalendarDef {
        id: CalendarId::new(),
        name: "Standard".into(),
        base: None,
        week: Some(week),
        exceptions: BTreeMap::new(),
        minutes_per_day: mpd,
    };
    let project = Project {
        id: ProjectId::new(),
        portfolio: None,
        name: name.to_owned(),
        data_date,
        planned_start: start,
        must_finish_by: None,
        default_calendar: cal.id,
        settings: ScheduleSettings {
            near_critical_threshold: WorkMinutes(5 * mpd),
            ..ScheduleSettings::default()
        },
    };
    let root = WbsNode {
        id: WbsId::new(),
        parent: None,
        code: name
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(12)
            .collect::<String>()
            .to_uppercase(),
        name: name.to_owned(),
        obs: None,
        sort_key: 0,
    };
    ProjectState::new(project, cal, root)
}

pub(crate) fn open(
    w: &mut Workspace,
    ctx: &Ctx,
    state: ProjectState,
) -> Result<ProjectId, EngineError> {
    ensure_name_free(w, &state.project.name)?;
    let repo = Repo::new(state, ctx.author.clone(), w.now())?;
    let pid = repo.project;
    w.repos.insert(pid, repo);
    Ok(pid)
}

/// Project names address projects, so two open projects may not share one.
fn ensure_name_free(w: &Workspace, name: &str) -> Result<(), EngineError> {
    if w.repos
        .values()
        .any(|r| r.branch(r.live).is_ok_and(|b| b.head.project.name == name))
    {
        return Err(EngineError::validation(format!(
            "a project named `{name}` is already open"
        )));
    }
    Ok(())
}

pub(crate) fn week_preset(s: &str) -> Result<(WorkWeek, i64), EngineError> {
    Ok(match s {
        "5x8" => (WorkWeek::standard_5x8(), 480),
        "6x10" => {
            let mut wk = WorkWeek::default();
            for d in &mut wk.days[..6] {
                *d = vec![DayInterval::hm(7, 0, 12, 0), DayInterval::hm(13, 0, 18, 0)];
            }
            (wk, 600)
        }
        "7x24" => (WorkWeek::continuous(), 1440),
        other => {
            return Err(EngineError::validation(format!(
                "unknown calendar preset `{other}` (5x8, 6x10, 7x24)"
            )));
        }
    })
}

pub fn list(w: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let brief = P::of(v)?.opt_bool("brief")?.unwrap_or(false);
    let mut out = Vec::new();
    let ids: Vec<ProjectId> = w.repos.keys().copied().collect();
    for pid in ids {
        let live = w.repo_ref(pid)?.live;
        let s = if brief {
            None
        } else {
            w.schedule(pid, live).ok()
        };
        let repo = w.repo_ref(pid)?;
        let head = &repo.branch(live)?.head;
        out.push(json!({
            "project": pid,
            "name": head.project.name,
            "activities": head.activities.len(),
            "finish": s.map(|s| s.project_finish().to_string()),
            "sandboxes": repo.branches().filter(|b| b.meta.kind == prswt_state::BranchKind::Sandbox
                && b.meta.status == prswt_state::BranchStatus::Open).count(),
            "revision": revision(repo),
        }));
    }
    Ok(json!({ "projects": out }))
}

/// Change counter for "unsaved changes": grows with every event on any
/// branch and with every branch that is merged, discarded or expired.
fn revision(repo: &Repo) -> u64 {
    repo.branches()
        .map(|b| {
            b.meta
                .version
                .saturating_add(u64::from(b.meta.status != prswt_state::BranchStatus::Open))
        })
        .fold(0u64, u64::saturating_add)
        .saturating_add(repo.branches().count() as u64)
}

/// DSK-06: codes and names of everything a form can pick (activities,
/// WBS, resources, calendars, risks, baselines), sorted by code, so the UI
/// offers lists instead of free-text fields.
pub fn lookups(w: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let repo = w.repo_ref(pid)?;
    let h = &repo.branch(bid)?.head;
    let sandboxes: Vec<(String, String)> = repo
        .branches()
        .filter(|b| {
            b.meta.kind == prswt_state::BranchKind::Sandbox
                && b.meta.status == prswt_state::BranchStatus::Open
        })
        .map(|b| (b.meta.name.clone(), b.meta.name.clone()))
        .collect();
    let sorted = |mut rows: Vec<(String, String)>| {
        rows.sort();
        rows.into_iter()
            .map(|(code, name)| json!({ "code": code, "name": name }))
            .collect::<Vec<_>>()
    };
    Ok(json!({
        "activities": sorted(h.activities.values().map(|a| (a.code.clone(), a.name.clone())).collect()),
        "wbs": sorted(h.wbs.values().map(|n| (n.code.clone(), n.name.clone())).collect()),
        "resources": sorted(h.resources.values().map(|r| (r.code.clone(), r.name.clone())).collect()),
        "calendars": sorted(h.calendars.values().map(|c| (c.name.clone(), c.name.clone())).collect()),
        "risks": sorted(h.risks.values().map(|r| (r.code.clone(), r.title.clone())).collect()),
        "baselines": sorted(h.baselines.values().map(|b| (b.name.clone(), b.name.clone())).collect()),
        "sandboxes": sorted(sandboxes),
    }))
}

/// DSK-02: removes a project from the workspace. Humans only: an agent must
/// not drop the plan someone is looking at (unsaved work would be lost).
pub fn close(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    if matches!(ctx.author, prswt_state::Author::Agent(_)) {
        return Err(EngineError::new(
            ErrorCode::LiveWriteForbidden,
            "agents cannot close projects",
        ));
    }
    let p = P::of(v)?;
    let pid = w.project_id(&p)?;
    w.sync.remove(&pid);
    let repo = w
        .repos
        .remove(&pid)
        .ok_or_else(|| EngineError::not_found(format!("project {pid}")))?;
    let name = repo.branch(repo.live)?.head.project.name.clone();
    Ok(json!({ "project": pid, "name": name, "closed": true }))
}

pub fn update(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let head = &w.repo_ref(pid)?.branch(bid)?.head;
    let mpd = head
        .calendars
        .get(&head.project.default_calendar)
        .map_or(480, |c| c.minutes_per_day);
    let mut settings = head.project.settings;
    if let Some(f) = p.opt_duration("critical_float", mpd)? {
        settings.critical = prswt_core::model::CriticalDefinition::TotalFloatAtMost(f);
    }
    if p.opt_str("critical_float")? == Some("longest_path") {
        settings.critical = prswt_core::model::CriticalDefinition::LongestPath;
    }
    if let Some(f) = p.opt_duration("near_critical_float", mpd)? {
        settings.near_critical_threshold = f;
    }
    if let Some(l) = p.opt_str("lag_calendar")? {
        settings.lag_calendar = match l {
            "predecessor" => LagCalendar::Predecessor,
            "successor" => LagCalendar::Successor,
            "project" => LagCalendar::Project,
            "24h" => LagCalendar::TwentyFourHour,
            _ => {
                return Err(EngineError::validation(
                    "lag_calendar: predecessor|successor|project|24h",
                ));
            }
        };
    }
    if let Some(m) = p.opt_str("progress_mode")? {
        settings.progress_mode = match m {
            "retained_logic" => ProgressMode::RetainedLogic,
            "progress_override" => ProgressMode::ProgressOverride,
            _ => {
                return Err(EngineError::validation(
                    "progress_mode: retained_logic|progress_override",
                ));
            }
        };
    }
    let cmd = Command::UpdateProject {
        name: p.opt_str("name")?.map(str::to_owned),
        data_date: p.opt_instant("data_date")?,
        planned_start: p.opt_instant("planned_start")?,
        must_finish_by: p.opt_instant("must_finish_by")?,
        clear_must_finish_by: p.bool_or("clear_must_finish_by", false)?,
        settings: Some(settings),
    };
    let e = w.apply(ctx, pid, bid, vec![cmd], p.opt_u64("expected_version")?)?;
    Ok(json!({ "version": e.version }))
}

pub fn sample(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let name = p.str("name")?;
    let state = samples::build(name)?;
    let pid = open(w, ctx, state)?;
    let live = w.repo_ref(pid)?.live;
    // Opening a sample creates a new project rather than editing someone's
    // live plan, so its initial baseline is recorded by the system even when
    // an agent asked for the sample.
    let system = Ctx {
        author: prswt_state::Author::System,
        access: None,
        account: None,
        read_only: false,
    };
    let bl = super::analysis::capture(w, &system, pid, live, "BL1")?;
    let s = w.schedule(pid, live)?;
    Ok(
        json!({ "project": pid, "name": name, "finish": s.project_finish().to_string(), "baseline": bl["baseline"] }),
    )
}

pub fn commands(_: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let q = p.opt_str("search")?.map(str::to_lowercase);
    let list: Vec<Value> = catalog()
        .iter()
        .filter(|c| {
            q.as_ref().is_none_or(|q| {
                c.id.contains(q.as_str()) || c.label.to_lowercase().contains(q.as_str())
            })
        })
        .map(|c| json!({ "id": c.id, "label": c.label, "menu": c.menu, "params": c.params, "writes": c.writes }))
        .collect();
    Ok(json!({ "commands": list }))
}

pub fn apply_raw(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let commands: Vec<Command> = p.req("commands")?;
    let e = w.apply(ctx, pid, bid, commands, p.opt_u64("expected_version")?)?;
    Ok(json!({ "version": e.version, "changes": e.changes.len() }))
}

pub fn undo(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    history_step(w, ctx, v, true)
}

pub fn redo(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    history_step(w, ctx, v, false)
}

fn history_step(w: &mut Workspace, ctx: &Ctx, v: &Value, undo: bool) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    w.guard_write(ctx, pid, bid)?;
    let at = w.now();
    let repo = w.repo_mut(pid)?;
    let e = if undo {
        repo.undo(bid, ctx.author.clone(), at)?
    } else {
        repo.redo(bid, ctx.author.clone(), at)?
    };
    w.emit(pid, &e);
    Ok(json!({ "version": e.version, "changes": e.changes.len() }))
}

pub fn history(w: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let since = p.opt_u64("since")?.unwrap_or(0);
    let b = w.repo_ref(pid)?.branch(bid)?;
    let events: Vec<Value> = b
        .events
        .iter()
        .filter(|e| e.version > since)
        .map(|e| {
            json!({
                "version": e.version,
                "at": e.at.to_string(),
                "author": e.author,
                "kind": match &e.kind {
                    prswt_state::EventKind::Commands { commands } => json!({ "commands": commands.len() }),
                    other => serde_json::to_value(other).unwrap_or(Value::Null),
                },
                "changed": e.changes.iter().map(|c| format!("{:?}:{}", c.entity.kind, c.entity.id)).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(json!({ "branch": b.meta.name, "version": b.meta.version, "events": events }))
}

/// Imports a P6 XER (text only: agents never get file-system access).
pub fn import_xer(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let text = p
        .raw("text")
        .and_then(Value::as_str)
        .ok_or_else(|| EngineError::validation("missing `text` (XER content)"))?;
    let state = prswt_formats::import(text)?;
    let (name, acts, links) = (
        state.project.name.clone(),
        state.activities.len(),
        state.relationships.len(),
    );
    let pid = open(w, ctx, state)?;
    Ok(json!({ "project": pid, "name": name, "activities": acts, "relationships": links }))
}

/// Exports a branch as P6 XER text.
pub fn export_xer(w: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let text = prswt_formats::export(&w.repo_ref(pid)?.branch(bid)?.head);
    Ok(json!({ "format": "xer", "bytes": text.len(), "text": text }))
}

/// Imports Microsoft Project XML (text only: agents never get file-system access).
pub fn import_mspdi(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let text = p
        .raw("text")
        .and_then(Value::as_str)
        .ok_or_else(|| EngineError::validation("missing `text` (MSPDI XML)"))?;
    let state = prswt_formats::mspdi::import(text)?;
    let (name, acts, links) = (
        state.project.name.clone(),
        state.activities.len(),
        state.relationships.len(),
    );
    let pid = open(w, ctx, state)?;
    Ok(json!({ "project": pid, "name": name, "activities": acts, "relationships": links }))
}

/// DSK-01: opens a `.prswt` project file (text; the desktop shell reads the
/// file). Branches, sandboxes and history come back; undo starts empty.
pub fn open_project(w: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let text = p
        .raw("text")
        .and_then(Value::as_str)
        .ok_or_else(|| EngineError::validation("missing `text` (project file content)"))?;
    if text.len() > MAX_FILE_BYTES {
        return Err(EngineError::new(
            ErrorCode::Limit,
            format!("project file is larger than {MAX_FILE_BYTES} bytes"),
        ));
    }
    let mut file: ProjectFile = serde_json::from_str(text)
        .map_err(|e| EngineError::validation(format!("not a readable project file: {e}")))?;
    let sync = file.sync.take();
    let repo = Repo::from_file(file)?;
    if w.repos.contains_key(&repo.project) {
        return Err(EngineError::validation("this project is already open"));
    }
    let live = repo.branch(repo.live)?;
    let name = live.head.project.name.clone();
    ensure_name_free(w, &name)?;
    let (acts, branches) = (live.head.activities.len(), repo.branches().count());
    let pid = repo.project;
    w.repos.insert(pid, repo);
    // A cached cloud copy remembers its server and how far it is synced;
    // a file shared by hand opens as a plain local project.
    // The link comes from a file, so it is checked like `sync.link`: an
    // http(s) server, and only branches the file has, at versions it has.
    let linked = match (sync, p.bool_or("keep_sync", false)?) {
        (Some(mut link), true)
            if link.server.starts_with("https://") || link.server.starts_with("http://") =>
        {
            let repo = w.repo_ref(pid)?;
            let have: std::collections::BTreeMap<_, _> = repo
                .branches()
                .map(|b| (b.meta.id, b.meta.version))
                .collect();
            link.synced.retain(|b, v| match have.get(b) {
                Some(max) => {
                    *v = (*v).min(*max);
                    true
                }
                None => false,
            });
            link.remote.retain(|b, _| have.contains_key(b));
            w.sync.insert(pid, link);
            true
        }
        _ => false,
    };
    Ok(
        json!({ "project": pid, "name": name, "activities": acts, "branches": branches, "linked": linked }),
    )
}

/// DSK-01: the whole project (every branch and its history) as `.prswt` text.
pub fn save_project(w: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let pid = w.project_id(&p)?;
    let repo = w.repo_ref(pid)?;
    let name = repo.branch(repo.live)?.head.project.name.clone();
    let mut file = repo.to_file();
    if p.bool_or("with_sync", false)? {
        file.sync = w.sync.get(&pid).cloned();
    }
    let text = serde_json::to_string(&file)
        .map_err(|e| EngineError::new(ErrorCode::Internal, format!("project file: {e}")))?;
    Ok(
        json!({ "format": "prswt", "project": pid, "name": name, "bytes": text.len(), "text": text }),
    )
}

/// Exports a branch as Microsoft Project XML text.
pub fn export_mspdi(w: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let text = prswt_formats::mspdi::export(&w.repo_ref(pid)?.branch(bid)?.head);
    Ok(json!({ "format": "mspdi", "bytes": text.len(), "text": text }))
}
