//! SYNC-01..03: cloud projects (ADR-0010). Sync exchanges branch changes
//! between a local copy (desktop) and the server, both running this engine.
//!
//! * Server side: `sync.events` hands out events since given versions;
//!   `sync.receive` takes a branch's local changes as whole-entity
//!   commands, re-validates them against the server head (optimistic
//!   concurrency on the version) and creates sandboxes made offline.
//! * Client side: `sync.link` marks a copy as a cloud project,
//!   `sync.outbox` lists what is not sent yet, `sync.apply` takes the
//!   server's events and rebases local work onto them ([`prswt_state::rebase`]):
//!   a field both sides changed keeps the server's value and is reported.
//!
//! The desktop shell moves the JSON over HTTP; nothing here does IO, so the
//! whole protocol is tested with two workspaces in one process.

use std::collections::BTreeMap;

use prswt_core::ids::{BranchId, ProjectId};
use prswt_core::{EngineError, ErrorCode};
use prswt_state::{
    Branch, BranchKind, BranchMeta, BranchStatus, Command, Event, ProjectState, Proposal, Repo,
    SyncLink, rebase, sync_commands,
};
use serde_json::{Value, json};

use crate::params::P;
use crate::views::t;
use crate::{Ctx, Workspace};

/// Most branches one `sync.apply` / `sync.events` call handles.
const MAX_SYNC_BRANCHES: usize = 1_000;
/// Longest proposal note kept (characters).
const MAX_NOTE: usize = 2_000;
/// Most events one branch may bring in one `sync.apply` call.
const MAX_SYNC_EVENTS: usize = 100_000;
/// Longest branch name kept (characters), as for `sandbox.create`.
const MAX_NAME: usize = 80;

/// `sync.link`: marks an open project as the local copy of a cloud project;
/// everything it holds now counts as in sync.
pub fn link(w: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let pid = w.project_id(&p)?;
    let server = p.str("server")?.trim_end_matches('/').to_owned();
    if !(server.starts_with("https://") || server.starts_with("http://")) {
        return Err(EngineError::validation(
            "`server` must be an http(s) origin",
        ));
    }
    let synced = w
        .repo_ref(pid)?
        .branches()
        .map(|b| (b.meta.id, b.meta.version))
        .collect();
    let remote = w
        .repo_ref(pid)?
        .branches()
        .map(|b| (b.meta.id, (b.meta.status, b.meta.proposal.clone())))
        .collect();
    w.sync.insert(
        pid,
        SyncLink {
            server: server.clone(),
            synced,
            remote,
        },
    );
    Ok(json!({ "project": pid, "server": server, "linked": true }))
}

/// `sync.status`: whether a project is a cloud copy and what is unsent.
pub fn status(w: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let pid = w.project_id(&p)?;
    let Some(link) = w.sync.get(&pid) else {
        return Ok(json!({ "project": pid, "linked": false }));
    };
    let repo = w.repo_ref(pid)?;
    let remote = Some(&link.remote);
    let rows: Vec<Value> = repo
        .branches()
        .map(|b| {
            let synced = link.synced.get(&b.meta.id).copied();
            json!({
                "id": b.meta.id,
                "name": b.meta.name,
                "kind": b.meta.kind,
                "status": b.meta.status,
                "version": b.meta.version,
                "synced": synced,
                "pending": pending(b, synced, remote),
                "proposal": b.meta.proposal.as_ref().map(proposal_json),
            })
        })
        .collect();
    let unsent = rows.iter().filter(|r| r["pending"] == true).count();
    Ok(json!({
        "project": pid,
        "linked": true,
        "server": link.server,
        "pending_branches": unsent,
        "branches": rows,
    }))
}

/// `sync.outbox`: every branch with unsent work: its metadata, the version
/// it builds on, its fork state when the server has never seen it, and the
/// commands that carry the changes.
pub fn outbox(w: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let pid = w.project_id(&p)?;
    let link = w
        .sync
        .get(&pid)
        .ok_or_else(|| EngineError::validation("this project is not a cloud project"))?;
    let repo = w.repo_ref(pid)?;
    let remote = Some(&link.remote);
    let mut out = Vec::new();
    // Live first: sandboxes made offline fork from it.
    let mut branches: Vec<&Branch> = repo.branches().collect();
    branches.sort_by_key(|b| b.meta.kind != BranchKind::Live);
    for b in branches {
        let synced = link.synced.get(&b.meta.id).copied();
        if !pending(b, synced, remote) {
            continue;
        }
        let (base_version, from) = match synced {
            Some(s) => (s, repo.state_at(b.meta.id, s)?),
            None => (0, b.base.clone()),
        };
        let commands = sync_commands(&from, &b.head);
        out.push(json!({
            "meta": b.meta,
            "base": if synced.is_none() { serde_json::to_value(&b.base).unwrap_or(Value::Null) } else { Value::Null },
            "base_version": base_version,
            "commands": commands,
        }));
    }
    Ok(json!({ "project": pid, "branches": out }))
}

/// `sync.receive` (server): applies one branch's local changes. The branch
/// must still be at `base_version` (else `VERSION_CONFLICT`: pull first). A
/// sandbox the server has not seen is created from the sent fork state.
/// Returns the branch's events after `base_version` for the sender to adopt.
pub fn receive(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let pid = w.project_id(&p)?;
    let meta: BranchMeta = p.req("meta")?;
    let base_version = p.opt_u64("base_version")?.unwrap_or(0);
    let commands: Vec<prswt_state::Command> = p.opt("commands")?.unwrap_or_default();
    let id = meta.id;
    if w.repo_ref(pid)?.branch(id).is_err() {
        create_branch(w, ctx, pid, &meta, p.raw("base"))?;
    }
    let current = w.repo_ref(pid)?.branch(id)?.meta.version;
    if current != base_version {
        return Err(EngineError::new(
            ErrorCode::VersionConflict,
            format!(
                "branch `{}` is at version {current} on the server, not {base_version}; pull first",
                meta.name
            ),
        )
        .with_details(json!({ "current": current })));
    }
    if commands.len() > prswt_state::MAX_BATCH {
        return Err(EngineError::new(
            ErrorCode::Limit,
            "too many changes in one branch",
        ));
    }
    if !commands.is_empty() {
        w.apply(ctx, pid, id, commands, Some(base_version))?;
    }
    adopt_state(w, ctx, pid, id, &meta)?;
    let branch = w.repo_ref(pid)?.branch(id)?;
    let events: Vec<&Event> = branch
        .events
        .iter()
        .filter(|e| e.version > base_version)
        .collect();
    Ok(json!({
        "project": pid,
        "meta": branch.meta,
        "base_version": base_version,
        "version": branch.meta.version,
        "events": events,
    }))
}

/// `sync.events` (server): each branch's metadata and its events after the
/// version the caller has; branches the caller lacks come with their fork
/// state and full log.
pub fn events(w: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let pid = w.project_id(&p)?;
    let since: BTreeMap<BranchId, u64> = p.opt("since")?.unwrap_or_default();
    let repo = w.repo_ref(pid)?;
    let rows: Vec<Value> = repo
        .branches()
        .take(MAX_SYNC_BRANCHES)
        .map(|b| {
            let from = since.get(&b.meta.id).copied();
            let events: Vec<&Event> = b
                .events
                .iter()
                .filter(|e| from.is_none_or(|f| e.version > f))
                .collect();
            json!({
                "meta": b.meta,
                "base": if from.is_none() { serde_json::to_value(&b.base).unwrap_or(Value::Null) } else { Value::Null },
                "events": events,
            })
        })
        .collect();
    Ok(json!({ "project": pid, "branches": rows }))
}

/// `sync.apply` (client): takes the server's branches (from `sync.events` or
/// a `sync.receive` reply) and rebases unsent local work onto them.
pub fn apply(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let pid = w.project_id(&p)?;
    if !w.sync.contains_key(&pid) {
        return Err(EngineError::validation(
            "this project is not a cloud project",
        ));
    }
    let rows: Vec<Value> = p.opt("branches")?.unwrap_or_default();
    if rows.len() > MAX_SYNC_BRANCHES {
        return Err(EngineError::new(ErrorCode::Limit, "too many branches"));
    }
    let mut conflicts = Vec::new();
    let mut updated = Vec::new();
    for row in rows {
        let rp = P::of(&row)?;
        let meta: BranchMeta = rp.req("meta")?;
        let events: Vec<Event> = rp.opt("events")?.unwrap_or_default();
        if events.len() > MAX_SYNC_EVENTS {
            return Err(EngineError::new(
                ErrorCode::Limit,
                "too many events for one branch",
            ));
        }
        let base: Option<ProjectState> = rp.opt("base")?;
        if let Some(found) = apply_branch(w, ctx, pid, &meta, base, events)? {
            conflicts.extend(found);
        }
        updated.push(meta.name.clone());
    }
    let pending = status(w, ctx, &json!({ "project": pid }))?["pending_branches"].clone();
    Ok(json!({
        "project": pid,
        "updated": updated,
        "conflicts": conflicts,
        "pending_branches": pending,
    }))
}

/// `sandbox.propose`: offers a sandbox for review ("commit this to live").
/// People and agents may propose; committing still needs a person.
pub fn propose(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let pid = w.project_id(&p)?;
    let sb = w.branch_id(pid, &p, "sandbox")?;
    let b = w.repo_ref(pid)?.branch(sb)?;
    if b.meta.kind == BranchKind::Live {
        return Err(EngineError::validation("only a sandbox can be proposed"));
    }
    let status = b.meta.status;
    if status != BranchStatus::Open {
        return Err(EngineError::validation("this sandbox is no longer open"));
    }
    let withdraw = p.bool_or("withdraw", false)?;
    let proposal = (!withdraw).then(|| Proposal {
        by: ctx.author.clone(),
        note: p
            .opt_str("note")
            .ok()
            .flatten()
            .unwrap_or("")
            .chars()
            .take(MAX_NOTE)
            .collect(),
        at: w.now(),
    });
    w.repo_mut(pid)?
        .set_branch_state(sb, status, proposal.clone())?;
    Ok(json!({
        "sandbox": sb,
        "proposed": proposal.is_some(),
        "proposal": proposal.as_ref().map(proposal_json),
    }))
}

/// Integrates one server branch into the local copy.
fn apply_branch(
    w: &mut Workspace,
    ctx: &Ctx,
    pid: ProjectId,
    meta: &BranchMeta,
    base: Option<ProjectState>,
    events: Vec<Event>,
) -> Result<Option<Vec<Value>>, EngineError> {
    let id = meta.id;
    let exists = w.repo_ref(pid)?.branch(id).is_ok();
    if !exists {
        // A branch the server has and this copy lacks: only a sandbox of
        // this project, forked from a branch here, with a sound fork state.
        let base = base.ok_or_else(|| {
            EngineError::validation(format!(
                "branch `{}` arrived without its fork state",
                meta.name
            ))
        })?;
        let repo = w.repo_ref(pid)?;
        let parent_ok = meta.parent.is_some_and(|p| repo.branch(p).is_ok());
        if meta.kind != BranchKind::Sandbox || !parent_ok {
            return Err(EngineError::validation(format!(
                "branch `{}` is not a sandbox of this project",
                meta.name
            )));
        }
        if base.project.id != pid {
            return Err(EngineError::validation(
                "a branch belongs to another project",
            ));
        }
        base.check_integrity()?;
        check_events(&events, id, 0)?;
        let mut m = meta.clone();
        m.version = 0;
        m.name = m.name.chars().take(MAX_NAME).collect();
        if let Some(pr) = &mut m.proposal {
            pr.note = pr.note.chars().take(MAX_NOTE).collect();
        }
        let repo = w.repo_mut(pid)?;
        repo.insert_branch(Branch::restored(m, base.clone(), base));
        load_events(repo, id, events)?;
        let version = repo.branch(id)?.meta.version;
        set_synced(w, pid, id, version, meta);
        return Ok(None);
    }
    let synced = w
        .sync
        .get(&pid)
        .and_then(|l| l.synced.get(&id))
        .copied()
        .unwrap_or(0);
    // Checked before anything is rewound (a failing call is also undone
    // as a whole by `execute`).
    check_events(&events, id, synced)?;
    // Versions are rewound and replayed: cached schedules would be stale.
    w.cache.clear();
    let repo = w.repo_mut(pid)?;
    let local_version = repo.branch(id)?.meta.version;
    let mut conflicts = None;
    if local_version > synced {
        // Local work to keep: rewind, take the server's events, re-apply.
        let agreed = repo.state_at(id, synced)?;
        let local = repo.branch(id)?.head.clone();
        repo.reset_branch(id, synced)?;
        load_events(repo, id, events)?;
        let server = repo.branch(id)?.head.clone();
        let (merged, mut found) = rebase(&agreed, &server, &local)?;
        let version = repo.branch(id)?.meta.version;
        let (commands, dropped) = applicable(&server, sync_commands(&server, &merged));
        if !commands.is_empty() {
            let at = w.now();
            let event = w
                .repo_mut(pid)?
                .execute(id, commands, ctx.author.clone(), at, None)?;
            w.emit(pid, &event);
        }
        set_synced(w, pid, id, version, meta);
        let mut rows: Vec<Value> = found
            .drain(..)
            .map(|c| {
                json!({
                    "branch": meta.name,
                    "entity": c.entity,
                    "field": c.field,
                    "server": c.live,
                    "yours": c.sandbox,
                })
            })
            .collect();
        // Local changes that no longer fit (an edit to something deleted
        // on the server): reported, never silently lost.
        rows.extend(dropped.into_iter().map(|(entity, yours)| {
            json!({
                "branch": meta.name,
                "entity": entity,
                "field": "*",
                "server": Value::Null,
                "yours": yours,
            })
        }));
        conflicts = Some(rows);
    } else {
        let appended = !events.is_empty() && events.iter().any(|e| e.version > local_version);
        load_events(repo, id, events)?;
        repo.branch(id)?.head.check_integrity()?;
        if appended {
            // Someone else's events are on top: undoing an older local
            // event would overwrite them.
            repo.forget_history(id)?;
        }
        let version = repo.branch(id)?.meta.version;
        set_synced(w, pid, id, version, meta);
    }
    Ok(conflicts)
}

/// Server events for branch `id` must be filed under it and continue
/// without gaps after the version both sides agree on.
fn check_events(events: &[Event], id: BranchId, synced: u64) -> Result<(), EngineError> {
    let mut want = synced;
    for e in events {
        if e.branch != id {
            return Err(EngineError::validation(
                "event filed under the wrong branch",
            ));
        }
        if e.version <= synced {
            continue;
        }
        want = want.saturating_add(1);
        if e.version != want {
            return Err(EngineError::validation(format!(
                "server events skip from version {} to {}",
                want.saturating_sub(1),
                e.version
            )));
        }
    }
    Ok(())
}

/// Splits rebased commands into those that still fit the server's state
/// and the entities whose local change no longer can (with the local
/// value). Normally all fit at once; otherwise each is tried in turn,
/// repeating while any more fit (one may need another, e.g. a link needs
/// its activity).
fn applicable(
    server: &ProjectState,
    commands: Vec<Command>,
) -> (Vec<Command>, Vec<(Value, Value)>) {
    let mut all = server.clone();
    if commands.iter().all(|c| c.apply(&mut all).is_ok()) && all.check_integrity().is_ok() {
        return (commands, Vec::new());
    }
    let mut state = server.clone();
    let mut kept = Vec::new();
    let mut left = commands;
    loop {
        let before = left.len();
        let mut rest = Vec::new();
        for c in left {
            let mut next = state.clone();
            if c.apply(&mut next).is_ok() && next.check_integrity().is_ok() {
                state = next;
                kept.push(c);
            } else {
                rest.push(c);
            }
        }
        left = rest;
        if left.is_empty() || left.len() == before {
            break;
        }
    }
    let dropped = left
        .into_iter()
        .map(|c| match c {
            Command::SetEntity { entity, value } => (
                serde_json::to_value(&entity).unwrap_or(Value::Null),
                value.unwrap_or(Value::Null),
            ),
            other => (
                Value::Null,
                serde_json::to_value(&other).unwrap_or(Value::Null),
            ),
        })
        .collect();
    (kept, dropped)
}

/// Appends server events the local copy does not have yet (checked with
/// [`check_events`]).
fn load_events(repo: &mut Repo, id: BranchId, events: Vec<Event>) -> Result<(), EngineError> {
    let have = repo.branch(id)?.meta.version;
    for e in events {
        if e.version > have {
            repo.apply_loaded_event(e)?;
        }
    }
    repo.branch(id)?.head.check_integrity()
}

/// Records the agreed version and the server's status for a branch; the
/// local status follows the server unless it was changed locally.
fn set_synced(w: &mut Workspace, pid: ProjectId, id: BranchId, version: u64, server: &BranchMeta) {
    let mut before = None;
    if let Some(link) = w.sync.get_mut(&pid) {
        link.synced.insert(id, version);
        before = link
            .remote
            .insert(id, (server.status, server.proposal.clone()));
    }
    if let Some(repo) = w.repos.get_mut(&pid) {
        let local = repo
            .branch(id)
            .map(|b| (b.meta.status, b.meta.proposal.clone()))
            .ok();
        // Unchanged locally since the last sync, or the server took the
        // same change (it records who proposed and when itself): take the
        // server's.
        let same = local.as_ref().is_some_and(|(status, proposal)| {
            *status == server.status
                && proposal.as_ref().map(|p| &p.note) == server.proposal.as_ref().map(|p| &p.note)
        });
        if local.is_some() && (local == before || same) {
            let _ = repo.set_branch_state(id, server.status, server.proposal.clone());
        }
    }
}

/// Server: makes a sandbox that was created offline on a client.
fn create_branch(
    w: &mut Workspace,
    ctx: &Ctx,
    pid: ProjectId,
    meta: &BranchMeta,
    base: Option<&Value>,
) -> Result<(), EngineError> {
    if meta.kind != BranchKind::Sandbox {
        return Err(EngineError::validation(
            "only sandboxes can be created by sync",
        ));
    }
    let name: String = meta.name.trim().chars().take(MAX_NAME).collect();
    if name.is_empty() || name == "live" {
        return Err(EngineError::validation(
            "a sandbox needs a name other than `live`",
        ));
    }
    let base: ProjectState =
        serde_json::from_value(base.cloned().ok_or_else(|| {
            EngineError::validation("a new sandbox needs `base` (its fork state)")
        })?)
        .map_err(|e| EngineError::validation(format!("bad `base`: {e}")))?;
    if base.project.id != pid {
        return Err(EngineError::validation("`base` belongs to another project"));
    }
    base.check_integrity()?;
    let repo = w.repo_ref(pid)?;
    let parent = meta.parent.unwrap_or(repo.live);
    let fork_version = repo.branch(parent)?.meta.version;
    if repo.find_by_name(&name).is_some() {
        return Err(EngineError::validation(format!(
            "an open branch named `{name}` exists"
        )));
    }
    if repo.branches().count() >= prswt_state::MAX_FILE_BRANCHES {
        return Err(EngineError::new(
            ErrorCode::Limit,
            "this project has too many branches",
        ));
    }
    let ttl = meta.ttl_minutes.map(|t| {
        t.clamp(
            0,
            i64::try_from(super::sandbox::MAX_TTL_MINUTES).unwrap_or(0),
        )
    });
    let m = BranchMeta {
        id: meta.id,
        name,
        kind: BranchKind::Sandbox,
        parent: Some(parent),
        fork_version,
        version: 0,
        status: BranchStatus::Open,
        created_by: ctx.author.clone(),
        created_at: w.now(),
        ttl_minutes: ttl,
        proposal: None,
    };
    w.repo_mut(pid)?
        .insert_branch(Branch::restored(m, base.clone(), base));
    Ok(())
}

/// Server: takes over a sandbox's status and review request from the
/// client (discarded, merged or proposed there). Only an open sandbox
/// changes status (a closed one is never reopened), and a review request
/// is recorded as made by the caller, now.
fn adopt_state(
    w: &mut Workspace,
    ctx: &Ctx,
    pid: ProjectId,
    id: BranchId,
    meta: &BranchMeta,
) -> Result<(), EngineError> {
    let b = w.repo_ref(pid)?.branch(id)?;
    if b.meta.kind == BranchKind::Live {
        return Ok(());
    }
    if b.meta.status == meta.status && b.meta.proposal == meta.proposal {
        return Ok(());
    }
    if b.meta.status != BranchStatus::Open {
        return Err(EngineError::validation(format!(
            "sandbox `{}` is already closed on the server",
            b.meta.name
        )));
    }
    let proposal_changed = b.meta.proposal.as_ref().map(|p| &p.note)
        != meta.proposal.as_ref().map(|p| &p.note)
        || b.meta.proposal.is_some() != meta.proposal.is_some();
    let kept = b.meta.proposal.clone();
    if meta.status == BranchStatus::Merged {
        // Committing to live is a person's decision (agent policy).
        if ctx.is_agent() && w.policy == crate::AgentPolicy::RequireHuman {
            return Err(EngineError::new(
                ErrorCode::LiveWriteForbidden,
                "committing to live needs a person",
            ));
        }
        // Live arrives first in a sync, so a real commit is already in it.
        if !w.repo_ref(pid)?.holds_sandbox(id)? {
            return Err(EngineError::validation(format!(
                "sandbox `{}` is marked merged, but live does not hold its changes; commit it again",
                b.meta.name
            )));
        }
    }
    let proposal = if proposal_changed {
        let at = w.now();
        meta.proposal.as_ref().map(|pr| Proposal {
            by: ctx.author.clone(),
            note: pr.note.chars().take(MAX_NOTE).collect(),
            at,
        })
    } else {
        kept
    };
    w.repo_mut(pid)?.set_branch_state(id, meta.status, proposal)
}

/// Unsent work on a branch: new to the server, events after the agreed
/// version, or a status / review request changed locally.
fn pending(
    b: &Branch,
    synced: Option<u64>,
    remote: Option<&BTreeMap<BranchId, (BranchStatus, Option<Proposal>)>>,
) -> bool {
    let Some(s) = synced else {
        return true;
    };
    if b.meta.version > s {
        return true;
    }
    remote
        .and_then(|r| r.get(&b.meta.id))
        .is_some_and(|(status, proposal)| *status != b.meta.status || *proposal != b.meta.proposal)
}

/// A review request for JSON replies.
fn proposal_json(p: &Proposal) -> Value {
    json!({ "by": p.by, "note": p.note, "at": t(p.at) })
}

/// Most conflict rows one `sync.reapply` takes.
const MAX_REAPPLY: usize = 1_000;

/// Puts back the local values a sync overwrote (the conflict rows it
/// reported): a field is set again on the current entity, a whole item
/// (`field` `*`) is restored as it was locally. Runs as one undoable change;
/// the next sync sends it like any other edit. An item that no longer fits
/// (e.g. a link to an activity deleted on the server) fails the whole call
/// with the integrity error, and nothing changes.
pub fn reapply(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let rows: Vec<Value> = p.req("conflicts")?;
    if rows.is_empty() || rows.len() > MAX_REAPPLY {
        return Err(EngineError::validation(format!(
            "conflicts: 1..={MAX_REAPPLY} rows"
        )));
    }
    let head = &w.repo_ref(pid)?.branch(bid)?.head;
    let mut cmds = Vec::with_capacity(rows.len());
    for row in &rows {
        let entity: prswt_state::EntityRef = serde_json::from_value(row["entity"].clone())
            .map_err(|e| EngineError::validation(format!("conflict entity: {e}")))?;
        let yours = row["yours"].clone();
        let value = match row["field"].as_str().unwrap_or("*") {
            "*" => (!yours.is_null()).then_some(yours),
            field => {
                let mut current = head.get_json(&entity).ok_or_else(|| {
                    EngineError::not_found(format!(
                        "{:?} {} (removed meanwhile)",
                        entity.kind, entity.id
                    ))
                })?;
                let Some(obj) = current.as_object_mut() else {
                    return Err(EngineError::validation("conflict entity is not an object"));
                };
                if !obj.contains_key(field) {
                    return Err(EngineError::validation(format!("unknown field `{field}`")));
                }
                obj.insert(field.to_owned(), yours);
                Some(current)
            }
        };
        cmds.push(Command::SetEntity { entity, value });
    }
    let e = w.apply(ctx, pid, bid, cmds, None)?;
    Ok(json!({ "version": e.version, "reapplied": rows.len() }))
}
