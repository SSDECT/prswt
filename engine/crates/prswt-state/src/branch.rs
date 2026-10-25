//! ST-03..08: branches (live + sandboxes), event log, undo/redo, diff and
//! three-way field-level merge.

use std::collections::{BTreeMap, BTreeSet};

use prswt_core::ids::{BranchId, ProjectId};
use prswt_core::{EngineError, ErrorCode, Instant};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::command::{Command, MAX_BATCH};
use crate::state::{EntityKind, EntityRef, ProjectState};

/// Undo history depth per branch.
pub const UNDO_DEPTH: usize = 100;

/// Who made a change.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "name")]
pub enum Author {
    /// A person (name).
    Human(String),
    /// An AI agent (name).
    Agent(String),
    /// The engine itself.
    System,
}

/// Branch role.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchKind {
    /// The project's real plan.
    Live,
    /// A what-if copy.
    Sandbox,
}

/// Branch lifecycle.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BranchStatus {
    /// Accepting changes.
    Open,
    /// Merged into its parent.
    Merged,
    /// Thrown away.
    Discarded,
    /// Its time-to-live passed.
    Expired,
}

/// How merge conflicts are resolved.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MergeStrategy {
    /// Abort on any conflict.
    RejectOnConflict,
    /// Take the sandbox's value.
    PreferSandbox,
    /// Keep the live value.
    PreferLive,
}

/// One entity's before/after, with the top-level fields that differ.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Change {
    /// Changed entity.
    pub entity: EntityRef,
    /// Changed top-level fields (`*` = created or removed).
    pub fields: Vec<String>,
    /// Value before (`None` = did not exist).
    pub before: Option<Value>,
    /// Value after (`None` = removed).
    pub after: Option<Value>,
}

/// What produced an event.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum EventKind {
    /// A command batch.
    Commands {
        /// The commands, in order.
        commands: Vec<Command>,
    },
    /// Undo of event `of`.
    Undo {
        /// Version that was undone.
        of: u64,
    },
    /// Redo of event `of`.
    Redo {
        /// Version that was redone.
        of: u64,
    },
    /// Merge of sandbox `from`.
    Merge {
        /// Sandbox that was merged.
        from: BranchId,
    },
}

/// One recorded change set on a branch.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Event {
    /// Branch.
    pub branch: BranchId,
    /// Version after the event (1, 2, …).
    pub version: u64,
    /// When it happened.
    pub at: Instant,
    /// Who did it.
    pub author: Author,
    /// What produced it.
    pub kind: EventKind,
    /// Entity changes.
    pub changes: Vec<Change>,
}

/// A field changed differently on live and in the sandbox.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Conflict {
    /// Entity.
    pub entity: EntityRef,
    /// Field name.
    pub field: String,
    /// Value at fork.
    pub base: Option<Value>,
    /// Live value.
    pub live: Option<Value>,
    /// Sandbox value.
    pub sandbox: Option<Value>,
}

/// SYNC-03: a sandbox offered for review ("please commit this to live").
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Proposal {
    /// Who proposed it (a person or an agent).
    pub by: Author,
    /// What it changes and why, in the proposer's words.
    pub note: String,
    /// When it was proposed.
    pub at: Instant,
}

/// Branch metadata.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct BranchMeta {
    /// Branch id.
    pub id: BranchId,
    /// Name (`live` or the sandbox name).
    pub name: String,
    /// Live or sandbox.
    pub kind: BranchKind,
    /// Branch it was forked from.
    pub parent: Option<BranchId>,
    /// Parent version at fork time.
    pub fork_version: u64,
    /// Current version.
    pub version: u64,
    /// Lifecycle status.
    pub status: BranchStatus,
    /// Creator.
    pub created_by: Author,
    /// Creation time.
    pub created_at: Instant,
    /// Minutes until auto-expiry.
    pub ttl_minutes: Option<i64>,
    /// Set while the sandbox waits for review (SYNC-03).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal: Option<Proposal>,
}

/// A branch: metadata, fork state, head state and event log.
#[derive(Clone, Debug)]
pub struct Branch {
    /// Metadata.
    pub meta: BranchMeta,
    /// State at fork (initial state for live).
    pub base: ProjectState,
    /// Current state.
    pub head: ProjectState,
    /// Events since the fork (all events for live).
    pub events: Vec<Event>,
    undo: Vec<u64>,
    redo: Vec<u64>,
}

impl Branch {
    /// Event with this version.
    pub fn event(&self, version: u64) -> Option<&Event> {
        self.events.iter().rev().find(|e| e.version == version)
    }

    /// Undoable / redoable step counts.
    pub fn history_depth(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }
}

/// All branches of one project.
#[derive(Clone, Debug)]
pub struct Repo {
    /// Project id.
    pub project: ProjectId,
    /// The live branch.
    pub live: BranchId,
    branches: BTreeMap<BranchId, Branch>,
}

impl Repo {
    /// New repository whose live branch starts at `initial` (integrity-checked).
    pub fn new(initial: ProjectState, author: Author, at: Instant) -> Result<Self, EngineError> {
        initial.check_integrity()?;
        let id = BranchId::new();
        let branch = Branch {
            meta: BranchMeta {
                id,
                name: "live".into(),
                kind: BranchKind::Live,
                parent: None,
                fork_version: 0,
                version: 0,
                status: BranchStatus::Open,
                created_by: author,
                created_at: at,
                ttl_minutes: None,
                proposal: None,
            },
            base: initial.clone(),
            head: initial,
            events: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
        };
        Ok(Self {
            project: branch.head.project.id,
            live: id,
            branches: BTreeMap::from([(id, branch)]),
        })
    }

    /// Rebuilds a repo from persisted parts (used by the store).
    pub fn from_parts(project: ProjectId, live: BranchId, branches: Vec<Branch>) -> Self {
        Self {
            project,
            live,
            branches: branches.into_iter().map(|b| (b.meta.id, b)).collect(),
        }
    }

    /// Branch by id.
    pub fn branch(&self, id: BranchId) -> Result<&Branch, EngineError> {
        self.branches
            .get(&id)
            .ok_or_else(|| EngineError::not_found(format!("branch {id}")))
    }

    fn branch_mut(&mut self, id: BranchId) -> Result<&mut Branch, EngineError> {
        self.branches
            .get_mut(&id)
            .ok_or_else(|| EngineError::not_found(format!("branch {id}")))
    }

    /// All branches.
    pub fn branches(&self) -> impl Iterator<Item = &Branch> {
        self.branches.values()
    }

    /// Open branch with this name.
    pub fn find_by_name(&self, name: &str) -> Option<BranchId> {
        self.branches
            .values()
            .find(|b| b.meta.name == name && b.meta.status == BranchStatus::Open)
            .map(|b| b.meta.id)
    }

    /// Atomic batch: all commands apply and the result passes integrity, or
    /// nothing changes.
    pub fn execute(
        &mut self,
        branch: BranchId,
        commands: Vec<Command>,
        author: Author,
        at: Instant,
        expected_version: Option<u64>,
    ) -> Result<Event, EngineError> {
        if commands.is_empty() {
            return Err(EngineError::validation("no commands"));
        }
        if commands.len() > MAX_BATCH {
            return Err(EngineError::new(
                ErrorCode::Limit,
                "too many commands in one batch",
            ));
        }
        let b = self.branch_mut(branch)?;
        ensure_open(b)?;
        if let Some(v) = expected_version
            && v != b.meta.version
        {
            return Err(EngineError::new(
                ErrorCode::VersionConflict,
                format!("branch is at version {}, expected {v}", b.meta.version),
            )
            .with_details(serde_json::json!({ "current": b.meta.version })));
        }
        let before = b.head.clone();
        let mut next = before.clone();
        let mut touched = BTreeSet::new();
        for c in &commands {
            touched.extend(c.apply(&mut next)?);
        }
        next.check_integrity()?;
        let changes = changes_between(&before, &next, &touched);
        let event = record(
            b,
            at,
            author,
            EventKind::Commands { commands },
            changes,
            next,
        );
        push_undo(b, event.version);
        b.redo.clear();
        Ok(event)
    }

    /// ST-08: reverts the latest undoable event, recorded as a new event.
    pub fn undo(
        &mut self,
        branch: BranchId,
        author: Author,
        at: Instant,
    ) -> Result<Event, EngineError> {
        let b = self.branch_mut(branch)?;
        ensure_open(b)?;
        let of = b
            .undo
            .last()
            .copied()
            .ok_or_else(|| EngineError::validation("nothing to undo"))?;
        let target = b
            .event(of)
            .ok_or_else(|| EngineError::not_found(format!("event {of}")))?
            .changes
            .clone();
        let next = patch(
            &b.head,
            target.iter().rev().map(|c| (&c.entity, c.before.clone())),
        )?;
        let touched = target.iter().map(|c| c.entity.clone()).collect();
        let changes = changes_between(&b.head, &next, &touched);
        let event = record(b, at, author, EventKind::Undo { of }, changes, next);
        b.undo.pop();
        b.redo.push(of);
        Ok(event)
    }

    /// Re-applies the most recently undone event, recorded as a new event.
    pub fn redo(
        &mut self,
        branch: BranchId,
        author: Author,
        at: Instant,
    ) -> Result<Event, EngineError> {
        let b = self.branch_mut(branch)?;
        ensure_open(b)?;
        let of = b
            .redo
            .last()
            .copied()
            .ok_or_else(|| EngineError::validation("nothing to redo"))?;
        let target = b
            .event(of)
            .ok_or_else(|| EngineError::not_found(format!("event {of}")))?
            .changes
            .clone();
        let next = patch(&b.head, target.iter().map(|c| (&c.entity, c.after.clone())))?;
        let touched = target.iter().map(|c| c.entity.clone()).collect();
        let changes = changes_between(&b.head, &next, &touched);
        let event = record(b, at, author, EventKind::Redo { of }, changes, next);
        b.redo.pop();
        push_undo(b, of);
        Ok(event)
    }

    /// ST-03: O(1) fork (structural sharing).
    pub fn fork(
        &mut self,
        from: BranchId,
        name: impl Into<String>,
        author: Author,
        at: Instant,
        ttl_minutes: Option<i64>,
    ) -> Result<BranchId, EngineError> {
        let parent = self.branch(from)?;
        ensure_open(parent)?;
        // A project file holds at most this many branches; beyond it the
        // project could no longer be saved and opened.
        if self.branches.len() >= crate::file::MAX_FILE_BRANCHES {
            return Err(EngineError::new(
                ErrorCode::Limit,
                format!(
                    "a project holds at most {} branches",
                    crate::file::MAX_FILE_BRANCHES
                ),
            ));
        }
        let id = BranchId::new();
        let branch = Branch {
            meta: BranchMeta {
                id,
                name: name.into(),
                kind: BranchKind::Sandbox,
                parent: Some(from),
                fork_version: parent.meta.version,
                version: 0,
                status: BranchStatus::Open,
                created_by: author,
                created_at: at,
                ttl_minutes,
                proposal: None,
            },
            base: parent.head.clone(),
            head: parent.head.clone(),
            events: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
        };
        self.branches.insert(id, branch);
        Ok(id)
    }

    /// ST-04: entity-level differences `head(b) − head(a)`.
    pub fn diff(&self, a: BranchId, b: BranchId) -> Result<Vec<Change>, EngineError> {
        Ok(diff_states(&self.branch(a)?.head, &self.branch(b)?.head))
    }

    /// ST-05: three-way merge of a sandbox into its parent.
    pub fn merge(
        &mut self,
        sandbox: BranchId,
        strategy: MergeStrategy,
        author: Author,
        at: Instant,
    ) -> Result<(Event, Vec<Conflict>), EngineError> {
        let sb = self.branch(sandbox)?;
        ensure_open(sb)?;
        let parent_id = sb.meta.parent.ok_or_else(|| {
            EngineError::validation("the live branch has no parent to merge into")
        })?;
        let parent = self.branch(parent_id)?;
        ensure_open(parent)?;
        let (merged, conflicts) = three_way(&sb.base, &parent.head, &sb.head, strategy)?;
        if strategy == MergeStrategy::RejectOnConflict && !conflicts.is_empty() {
            return Err(EngineError::new(
                ErrorCode::MergeConflict,
                format!("{} conflicting field(s)", conflicts.len()),
            )
            .with_details(serde_json::to_value(&conflicts).unwrap_or_default()));
        }
        merged.check_integrity()?;
        let changes = diff_states(&parent.head, &merged);
        let p = self.branch_mut(parent_id)?;
        let event = record(
            p,
            at,
            author,
            EventKind::Merge { from: sandbox },
            changes,
            merged,
        );
        push_undo(p, event.version);
        p.redo.clear();
        self.branch_mut(sandbox)?.meta.status = BranchStatus::Merged;
        Ok((event, conflicts))
    }

    /// Marks a sandbox discarded (live cannot be discarded).
    pub fn discard(&mut self, sandbox: BranchId) -> Result<(), EngineError> {
        let b = self.branch_mut(sandbox)?;
        if b.meta.kind == BranchKind::Live {
            return Err(EngineError::validation(
                "the live branch cannot be discarded",
            ));
        }
        b.meta.status = BranchStatus::Discarded;
        Ok(())
    }

    /// ST-06: expires open sandboxes whose TTL has passed; returns them.
    pub fn expire(&mut self, now: Instant) -> Vec<BranchId> {
        let mut out = Vec::new();
        for b in self.branches.values_mut() {
            let m = &mut b.meta;
            if m.status == BranchStatus::Open
                && m.kind == BranchKind::Sandbox
                && m.ttl_minutes
                    .is_some_and(|t| m.created_at.plus_minutes(t) <= now)
            {
                m.status = BranchStatus::Expired;
                out.push(m.id);
            }
        }
        out
    }

    /// Invariant 4: replaying the branch's recorded changes from its base
    /// reproduces its head.
    pub fn replay(&self, branch: BranchId) -> Result<ProjectState, EngineError> {
        let b = self.branch(branch)?;
        let mut s = b.base.clone();
        for e in &b.events {
            s = patch(&s, e.changes.iter().map(|c| (&c.entity, c.after.clone())))?;
        }
        Ok(s)
    }

    /// State of `branch` right after `version` (replay from its base).
    pub fn state_at(&self, branch: BranchId, version: u64) -> Result<ProjectState, EngineError> {
        let b = self.branch(branch)?;
        let mut s = b.base.clone();
        for e in b.events.iter().filter(|e| e.version <= version) {
            s = patch(&s, e.changes.iter().map(|c| (&c.entity, c.after.clone())))?;
        }
        Ok(s)
    }

    /// Appends an externally loaded event (store replay); trusts its changes.
    pub fn apply_loaded_event(&mut self, event: Event) -> Result<(), EngineError> {
        let b = self.branch_mut(event.branch)?;
        if event.version != b.meta.version + 1 {
            return Err(EngineError::new(
                ErrorCode::VersionConflict,
                "event out of order",
            ));
        }
        b.head = patch(
            &b.head,
            event.changes.iter().map(|c| (&c.entity, c.after.clone())),
        )?;
        b.meta.version = event.version;
        b.events.push(event);
        Ok(())
    }

    /// Adds a branch (store loading).
    pub fn insert_branch(&mut self, branch: Branch) {
        self.branches.insert(branch.meta.id, branch);
    }

    /// SYNC-02: drops every event after `version` and rewinds the head to
    /// that point (local changes about to be rebased onto the server's).
    /// Undo and redo start empty afterwards.
    pub fn reset_branch(&mut self, id: BranchId, version: u64) -> Result<(), EngineError> {
        let head = self.state_at(id, version)?;
        let b = self.branch_mut(id)?;
        if version > b.meta.version {
            return Err(EngineError::validation(format!(
                "branch is at version {}, cannot reset forward to {version}",
                b.meta.version
            )));
        }
        b.events.retain(|e| e.version <= version);
        b.head = head;
        b.meta.version = version;
        b.undo.clear();
        b.redo.clear();
        Ok(())
    }

    /// SYNC-02: forgets a branch's undo and redo history (events arrived
    /// from elsewhere, so undoing an older local event would overwrite them).
    pub fn forget_history(&mut self, id: BranchId) -> Result<(), EngineError> {
        let b = self.branch_mut(id)?;
        b.undo.clear();
        b.redo.clear();
        Ok(())
    }

    /// SYNC-02/03: replaces a branch's status and review request as given
    /// by the server (merged, discarded, proposed elsewhere).
    pub fn set_branch_state(
        &mut self,
        id: BranchId,
        status: BranchStatus,
        proposal: Option<Proposal>,
    ) -> Result<(), EngineError> {
        let b = self.branch_mut(id)?;
        b.meta.status = status;
        b.meta.proposal = proposal;
        Ok(())
    }
}

/// Records an undoable event, keeping at most [`UNDO_DEPTH`].
fn push_undo(b: &mut Branch, version: u64) {
    b.undo.push(version);
    if b.undo.len() > UNDO_DEPTH {
        b.undo.remove(0);
    }
}

/// SYNC-02: rebases local work onto the server. `base` is the state both
/// sides last agreed on, `server` the server's head now and `local` the
/// local head. Changes made only locally are applied to the server's state;
/// a field both sides changed differently keeps the server's value and is
/// reported as a conflict, so nobody else's work is overwritten silently.
pub fn rebase(
    base: &ProjectState,
    server: &ProjectState,
    local: &ProjectState,
) -> Result<(ProjectState, Vec<Conflict>), EngineError> {
    three_way(base, server, local, MergeStrategy::PreferLive)
}

/// SYNC-02: the commands that turn `from` into `to`, one whole-entity
/// [`Command::SetEntity`] per changed entity.
pub fn sync_commands(from: &ProjectState, to: &ProjectState) -> Vec<Command> {
    diff_states(from, to)
        .into_iter()
        .map(|c| Command::SetEntity {
            entity: c.entity,
            value: c.after,
        })
        .collect()
}

impl Branch {
    /// A branch rebuilt from storage with an empty event list.
    pub fn restored(meta: BranchMeta, base: ProjectState, head: ProjectState) -> Self {
        Self {
            meta,
            base,
            head,
            events: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }
}

fn ensure_open(b: &Branch) -> Result<(), EngineError> {
    if b.meta.status == BranchStatus::Open {
        Ok(())
    } else {
        Err(EngineError::validation(format!(
            "branch {} is {:?}",
            b.meta.name, b.meta.status
        )))
    }
}

fn record(
    b: &mut Branch,
    at: Instant,
    author: Author,
    kind: EventKind,
    changes: Vec<Change>,
    next: ProjectState,
) -> Event {
    b.meta.version += 1;
    let event = Event {
        branch: b.meta.id,
        version: b.meta.version,
        at,
        author,
        kind,
        changes,
    };
    b.head = next;
    b.events.push(event.clone());
    event
}

fn patch<'a>(
    s: &ProjectState,
    sets: impl Iterator<Item = (&'a EntityRef, Option<Value>)>,
) -> Result<ProjectState, EngineError> {
    let mut next = s.clone();
    for (r, v) in sets {
        next.set_json(r, v)?;
    }
    Ok(next)
}

fn changes_between(
    a: &ProjectState,
    b: &ProjectState,
    touched: &BTreeSet<EntityRef>,
) -> Vec<Change> {
    touched
        .iter()
        .filter_map(|r| {
            let (before, after) = (a.get_json(r), b.get_json(r));
            let fields = changed_fields(before.as_ref(), after.as_ref());
            (!fields.is_empty()).then(|| Change {
                entity: r.clone(),
                fields,
                before,
                after,
            })
        })
        .collect()
}

/// Every entity difference from `a` to `b`.
pub fn diff_states(a: &ProjectState, b: &ProjectState) -> Vec<Change> {
    let mut refs = BTreeSet::new();
    for kind in EntityKind::ALL {
        for k in a.keys(kind).into_iter().chain(b.keys(kind)) {
            refs.insert(EntityRef::new(kind, k));
        }
    }
    changes_between(a, b, &refs)
}

fn changed_fields(a: Option<&Value>, b: Option<&Value>) -> Vec<String> {
    match (a, b) {
        (None, None) => Vec::new(),
        (Some(x), Some(y)) if x == y => Vec::new(),
        (Some(Value::Object(x)), Some(Value::Object(y))) => {
            let keys: BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            keys.into_iter()
                .filter(|k| x.get(*k) != y.get(*k))
                .map(|k| (*k).clone())
                .collect()
        }
        _ => vec!["*".into()],
    }
}

/// Field-level merge: a field takes the side that changed it; both changed
/// to different values is a conflict resolved by `strategy`.
fn three_way(
    base: &ProjectState,
    live: &ProjectState,
    sandbox: &ProjectState,
    strategy: MergeStrategy,
) -> Result<(ProjectState, Vec<Conflict>), EngineError> {
    let mut merged = live.clone();
    let mut conflicts = Vec::new();
    for change in diff_states(base, sandbox) {
        let r = &change.entity;
        let (b, l, s) = (base.get_json(r), live.get_json(r), sandbox.get_json(r));
        if l == b {
            merged.set_json(r, s)?;
            continue;
        }
        if l == s {
            continue;
        }
        match (&b, &l, &s) {
            (Some(Value::Object(bo)), Some(Value::Object(lo)), Some(Value::Object(so))) => {
                let mut out: Map<String, Value> = lo.clone();
                let keys: BTreeSet<&String> = bo.keys().chain(lo.keys()).chain(so.keys()).collect();
                for k in keys {
                    let (bv, lv, sv) = (bo.get(k), lo.get(k), so.get(k));
                    let pick = if lv == bv {
                        sv
                    } else if sv == bv || lv == sv {
                        lv
                    } else {
                        conflicts.push(Conflict {
                            entity: r.clone(),
                            field: k.clone(),
                            base: bv.cloned(),
                            live: lv.cloned(),
                            sandbox: sv.cloned(),
                        });
                        match strategy {
                            MergeStrategy::PreferSandbox => sv,
                            MergeStrategy::PreferLive | MergeStrategy::RejectOnConflict => lv,
                        }
                    };
                    match pick {
                        Some(v) => {
                            out.insert(k.clone(), v.clone());
                        }
                        None => {
                            out.remove(k);
                        }
                    }
                }
                merged.set_json(r, Some(Value::Object(out)))?;
            }
            _ => {
                conflicts.push(Conflict {
                    entity: r.clone(),
                    field: "*".into(),
                    base: b.clone(),
                    live: l.clone(),
                    sandbox: s.clone(),
                });
                if strategy == MergeStrategy::PreferSandbox {
                    merged.set_json(r, s)?;
                }
            }
        }
    }
    Ok((merged, conflicts))
}
