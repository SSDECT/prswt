//! Service layer: the single write path ("everything is a command",
//! ADR-0007). UI, CLI, control channel and MCP all call
//! [`Workspace::execute`] with a command id and JSON params.
//!
//! Ids follow Primavera P6's menus/actions (`schedule.run` = F9,
//! `activity.add`, `baseline.capture`) plus PRSWT's own (`risk.realize`,
//! `simulation.run`, `sandbox.commit`).

mod cmds;
pub mod params;
pub mod samples;
mod views;

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use prswt_core::ids::{ActivityId, BranchId, ProjectId, SimulationRunId};
use prswt_core::{EngineError, ErrorCode, Instant};
use prswt_cpm::Schedule;
use prswt_risk::SimResult;
use prswt_state::{Author, BranchKind, Repo};
use serde::Serialize;
use serde_json::Value;

pub use cmds::catalog;
pub use prswt_state::{Author as Who, Command as StateCommand};

/// Compiled schedules kept per (branch, version).
const SCHEDULE_CACHE: usize = 32;
/// Simulation results kept in memory.
const SIM_CACHE: usize = 64;

/// Wall-clock source, injected by the host (the engine never reads the clock).
pub trait Clock: Send + Sync {
    /// Current instant in project-local time.
    fn now(&self) -> Instant;
}

/// Fixed clock for tests and deterministic replays.
#[derive(Debug, Clone, Copy)]
pub struct FixedClock(pub Instant);

impl Clock for FixedClock {
    fn now(&self) -> Instant {
        self.0
    }
}

/// Who may change the live branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPolicy {
    /// Agents work in sandboxes; committing to live needs a human.
    RequireHuman,
    /// Agents may commit sandboxes and write live directly.
    Allow,
}

/// One registered command.
#[derive(Clone, Copy)]
pub struct CommandSpec {
    /// Stable command id (P6-style, e.g. `schedule.run`).
    pub id: &'static str,
    /// Short human label for menus and tool lists.
    pub label: &'static str,
    /// P6-style menu path, e.g. "Tools ▸ Schedule (F9)".
    pub menu: &'static str,
    /// Parameter summary shown by `commands.list` and MCP.
    pub params: &'static str,
    /// Changes project state (subject to the agent policy).
    pub writes: bool,
    /// Handler: validates params, performs the action, returns JSON.
    pub run: fn(&mut Workspace, &Ctx, &Value) -> Result<Value, EngineError>,
}

impl std::fmt::Debug for CommandSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandSpec")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// Commands that make a new project rather than change one.
const CREATES: [&str; 5] = [
    "project.create",
    "sample.load",
    "file.open_project",
    "file.import_xer",
    "file.import_mspdi",
];

/// SYNC-02: commands only a local copy runs. On a shared server they would
/// write branches without the agent policy or the integrity checks (the
/// server's side of the protocol is `sync.events` / `sync.receive`).
const CLIENT_ONLY: [&str; 3] = ["sync.link", "sync.outbox", "sync.apply"];

/// SYNC-01: what a caller may do with one shared project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Sees the project and runs analyses; changes nothing.
    Viewer,
    /// Changes the project.
    Editor,
    /// Changes it, shares it and closes it.
    Owner,
}

/// The projects a caller may see, with their role on each.
pub type Access = BTreeMap<ProjectId, Role>;

/// Caller context.
#[derive(Debug, Clone)]
pub struct Ctx {
    /// Who is acting; recorded on every event.
    pub author: Author,
    /// SYNC-01: when set (shared server), the caller sees only these
    /// projects and acts with these roles; `None` (desktop, CLI) sees all.
    pub access: Option<Arc<Access>>,
    /// The signed-in account behind this caller (server only): the person,
    /// or the owner of an agent's token.
    pub account: Option<String>,
    /// SYNC-04: a read-only token: every write is refused, including
    /// creating projects (which would make the caller an owner who can
    /// invite people).
    pub read_only: bool,
}

impl Ctx {
    /// Context for a human user.
    pub fn human(name: &str) -> Self {
        Self {
            author: Author::Human(name.into()),
            access: None,
            account: None,
            read_only: false,
        }
    }

    /// Context for an AI agent (subject to [`AgentPolicy`]).
    pub fn agent(name: &str) -> Self {
        Self {
            author: Author::Agent(name.into()),
            access: None,
            account: None,
            read_only: false,
        }
    }

    /// The same caller, recorded as acting for `account`.
    #[must_use]
    pub fn for_account(mut self, account: &str) -> Self {
        self.account = Some(account.to_owned());
        self
    }

    /// The same caller with every write refused (read-only token).
    #[must_use]
    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    /// The same caller limited to `access` (multi-user server).
    #[must_use]
    pub fn with_access(mut self, access: Access) -> Self {
        self.access = Some(Arc::new(access));
        self
    }

    /// True when the caller is an agent.
    pub fn is_agent(&self) -> bool {
        matches!(self.author, Author::Agent(_))
    }
}

pub(crate) struct StoredSim {
    pub project: ProjectId,
    pub branch: BranchId,
    pub result: SimResult,
    pub summary: Value,
}

/// Hook run for each project a scoped caller creates ([`Workspace::on_create`]).
pub type OnCreate = Box<dyn FnMut(&Ctx, ProjectId) -> Result<(), EngineError> + Send>;

/// All open projects plus caches.
pub struct Workspace {
    pub(crate) repos: BTreeMap<ProjectId, Repo>,
    pub(crate) clock: Arc<dyn Clock>,
    /// Whether agents may change the live branch.
    pub policy: AgentPolicy,
    cache: Vec<((BranchId, u64), Arc<Schedule>)>,
    pub(crate) sims: Vec<(SimulationRunId, StoredSim)>,
    /// Baselines (EVM snapshots) by id; metadata lives in the project state.
    pub(crate) baselines: BTreeMap<prswt_core::ids::BaselineId, Arc<prswt_evm::Baseline>>,
    /// Hook called after every successful write (persistence).
    pub on_event: Option<Box<dyn FnMut(ProjectId, &prswt_state::Event) + Send>>,
    /// SYNC-01: called for each project a scoped caller creates (the server
    /// makes the caller its owner). An error undoes the command: a project
    /// nobody owns must not be left behind.
    pub on_create: Option<OnCreate>,
    /// SYNC-01: local copies of cloud projects and how far they are synced.
    pub(crate) sync: BTreeMap<ProjectId, prswt_state::SyncLink>,
}

impl std::fmt::Debug for Workspace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Workspace")
            .field("projects", &self.repos.len())
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

impl Workspace {
    /// Empty workspace using `clock` for event timestamps.
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            repos: BTreeMap::new(),
            clock,
            policy: AgentPolicy::RequireHuman,
            cache: Vec::new(),
            sims: Vec::new(),
            baselines: BTreeMap::new(),
            on_event: None,
            on_create: None,
            sync: BTreeMap::new(),
        }
    }

    /// Runs command `id`. Escaped panics are caught and the workspace is
    /// restored (never-crash guard); the error says what happened.
    pub fn execute(&mut self, ctx: &Ctx, id: &str, params: &Value) -> Result<Value, EngineError> {
        let spec = catalog()
            .iter()
            .find(|c| c.id == id)
            .copied()
            .ok_or_else(|| {
                EngineError::not_found(format!("command `{id}` (see `commands.list`)"))
            })?;
        let params = if params.is_null() {
            &Value::Object(serde_json::Map::new())
        } else {
            params
        };
        // SYNC-01: a scoped caller works on a workspace holding only its
        // projects, so lookups, "the only open project" and listings never
        // reach anyone else's. The others are put back afterwards.
        let hidden: Vec<(ProjectId, Repo)> = match &ctx.access {
            Some(access) => {
                let others: Vec<ProjectId> = self
                    .repos
                    .keys()
                    .filter(|k| !access.contains_key(k))
                    .copied()
                    .collect();
                others
                    .into_iter()
                    .filter_map(|k| self.repos.remove_entry(&k))
                    .collect()
            }
            None => Vec::new(),
        };
        let before: BTreeSet<ProjectId> = self.repos.keys().copied().collect();
        let result = match self.authorize(ctx, &spec, params) {
            Ok(()) => self.run_guarded(ctx, &spec, params),
            Err(e) => Err(e),
        };
        let created: Vec<ProjectId> = self
            .repos
            .keys()
            .filter(|k| !before.contains(k))
            .copied()
            .collect();
        let clash = created.iter().any(|c| hidden.iter().any(|(h, _)| h == c));
        let mut result = if clash {
            Err(EngineError::validation(
                "a project with this id already exists on this server",
            ))
        } else {
            result
        };
        if result.is_ok()
            && ctx.access.is_some()
            && let Some(hook) = self.on_create.as_mut()
        {
            for c in &created {
                if let Err(e) = hook(ctx, *c) {
                    result = Err(e);
                    break;
                }
            }
        }
        if result.is_err() {
            for c in &created {
                self.repos.remove(c);
                self.sync.remove(c);
            }
        }
        self.repos.extend(hidden);
        result
    }

    /// Runs a command, catching escaped panics. A write that fails or
    /// panics leaves the projects and their sync state as they were, so a
    /// half-done change (a rewound branch, a project without its baseline)
    /// is never kept.
    fn run_guarded(
        &mut self,
        ctx: &Ctx,
        spec: &CommandSpec,
        params: &Value,
    ) -> Result<Value, EngineError> {
        // Cheap: branch states share structure (persistent maps).
        let saved = (self.repos.clone(), self.sync.clone());
        let outcome = catch_unwind(AssertUnwindSafe(|| (spec.run)(self, ctx, params)));
        match outcome {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => {
                if spec.writes {
                    (self.repos, self.sync) = saved;
                    self.cache.clear();
                }
                Err(e)
            }
            Err(_) => {
                (self.repos, self.sync) = saved;
                self.cache.clear();
                Err(EngineError::new(
                    ErrorCode::Internal,
                    format!(
                        "command `{}` failed unexpectedly; the project was restored",
                        spec.id
                    ),
                ))
            }
        }
    }

    /// SYNC-01: role checks. Read-only callers write nothing. For a scoped
    /// caller (shared server): viewers may not write, local-copy commands
    /// and closing are refused, and a write must name a project it can see.
    /// Commands that create a project name none, so they pass (the creator
    /// becomes its owner).
    fn authorize(&self, ctx: &Ctx, spec: &CommandSpec, params: &Value) -> Result<(), EngineError> {
        if ctx.read_only && spec.writes {
            return Err(EngineError::new(
                ErrorCode::Forbidden,
                format!("this token is read-only; `{}` changes things", spec.id),
            ));
        }
        let Some(access) = &ctx.access else {
            return Ok(());
        };
        if CLIENT_ONLY.contains(&spec.id) || spec.id == "project.close" {
            // A server keeps its projects: closing one would only drop it
            // from memory while its rows stay (and could be taken over).
            return Err(EngineError::new(
                ErrorCode::Forbidden,
                format!("`{}` is not available on a shared server", spec.id),
            ));
        }
        if !spec.writes || CREATES.contains(&spec.id) {
            return Ok(());
        }
        let pid = params::P::of(params).and_then(|p| self.project_id(&p))?;
        let role = access.get(&pid).copied().unwrap_or(Role::Viewer);
        let needed = if spec.id == "project.close" {
            Role::Owner
        } else {
            Role::Editor
        };
        if role < needed {
            return Err(EngineError::new(
                ErrorCode::Forbidden,
                format!(
                    "your role on this project is {role:?}; `{}` needs {needed:?}",
                    spec.id
                ),
            ));
        }
        Ok(())
    }

    /// Every open project.
    pub fn projects(&self) -> impl Iterator<Item = &Repo> {
        self.repos.values()
    }

    /// One open project by id.
    pub fn repo(&self, id: ProjectId) -> Option<&Repo> {
        self.repos.get(&id)
    }

    /// Inserts a repo restored from storage.
    pub fn insert_repo(&mut self, repo: Repo) {
        self.repos.insert(repo.project, repo);
    }

    pub(crate) fn now(&self) -> Instant {
        self.clock.now()
    }

    // ── addressing ──

    /// `project` param: id or exact name; optional when exactly one project is open.
    pub(crate) fn project_id(&self, p: &params::P<'_>) -> Result<ProjectId, EngineError> {
        match p.opt_str("project")? {
            Some(s) => {
                if let Ok(id) = s.parse::<ProjectId>()
                    && self.repos.contains_key(&id)
                {
                    return Ok(id);
                }
                self.repos
                    .values()
                    .find(|r| r.branch(r.live).is_ok_and(|b| b.head.project.name == s))
                    .map(|r| r.project)
                    .ok_or_else(|| EngineError::not_found(format!("project `{s}`")))
            }
            None => match self.repos.len() {
                1 => self
                    .repos
                    .keys()
                    .next()
                    .copied()
                    .ok_or_else(|| EngineError::not_found("project")),
                0 => Err(EngineError::not_found(
                    "open project (create one with `project.create` or `sample.load`)",
                )),
                _ => Err(EngineError::validation(
                    "several projects are open; pass `project`",
                )),
            },
        }
    }

    pub(crate) fn repo_ref(&self, pid: ProjectId) -> Result<&Repo, EngineError> {
        self.repos
            .get(&pid)
            .ok_or_else(|| EngineError::not_found(format!("project {pid}")))
    }

    pub(crate) fn repo_mut(&mut self, pid: ProjectId) -> Result<&mut Repo, EngineError> {
        self.repos
            .get_mut(&pid)
            .ok_or_else(|| EngineError::not_found(format!("project {pid}")))
    }

    /// `branch` param: "live" (default), sandbox name or id.
    pub(crate) fn branch_id(
        &self,
        pid: ProjectId,
        p: &params::P<'_>,
        key: &str,
    ) -> Result<BranchId, EngineError> {
        let repo = self.repo_ref(pid)?;
        match p.opt_str(key)? {
            None | Some("live") => Ok(repo.live),
            Some(s) => {
                if let Ok(id) = s.parse::<BranchId>()
                    && repo.branch(id).is_ok()
                {
                    return Ok(id);
                }
                repo.find_by_name(s)
                    .ok_or_else(|| EngineError::not_found(format!("branch `{s}`")))
            }
        }
    }

    pub(crate) fn target(&self, p: &params::P<'_>) -> Result<(ProjectId, BranchId), EngineError> {
        let pid = self.project_id(p)?;
        Ok((pid, self.branch_id(pid, p, "branch")?))
    }

    /// Activity by id or code on a branch.
    pub(crate) fn activity_id(
        &self,
        pid: ProjectId,
        bid: BranchId,
        s: &str,
    ) -> Result<ActivityId, EngineError> {
        let head = &self.repo_ref(pid)?.branch(bid)?.head;
        if let Ok(id) = s.parse::<ActivityId>()
            && head.activities.contains_key(&id)
        {
            return Ok(id);
        }
        head.activities
            .values()
            .find(|a| a.code == s)
            .map(|a| a.id)
            .ok_or_else(|| EngineError::not_found(format!("activity `{s}`")))
    }

    /// Enforces the agent policy on writes to the live branch.
    pub(crate) fn guard_write(
        &self,
        ctx: &Ctx,
        pid: ProjectId,
        bid: BranchId,
    ) -> Result<(), EngineError> {
        let live = self.repo_ref(pid)?.branch(bid)?.meta.kind == BranchKind::Live;
        if live && ctx.is_agent() && self.policy == AgentPolicy::RequireHuman {
            return Err(EngineError::new(
                ErrorCode::LiveWriteForbidden,
                "agents change the live project only through a sandbox and a human-approved commit; \
                 use `sandbox.create`, work there, then ask a human to run `sandbox.commit`",
            ));
        }
        Ok(())
    }

    /// Applies a state-command batch and notifies the persistence hook.
    pub(crate) fn apply(
        &mut self,
        ctx: &Ctx,
        pid: ProjectId,
        bid: BranchId,
        commands: Vec<prswt_state::Command>,
        expected: Option<u64>,
    ) -> Result<prswt_state::Event, EngineError> {
        self.guard_write(ctx, pid, bid)?;
        let at = self.now();
        let event = self
            .repo_mut(pid)?
            .execute(bid, commands, ctx.author.clone(), at, expected)?;
        self.emit(pid, &event);
        Ok(event)
    }

    pub(crate) fn emit(&mut self, pid: ProjectId, event: &prswt_state::Event) {
        if let Some(hook) = self.on_event.as_mut() {
            hook(pid, event);
        }
    }

    /// Calculated schedule of a branch head, cached per version.
    pub(crate) fn schedule(
        &mut self,
        pid: ProjectId,
        bid: BranchId,
    ) -> Result<Arc<Schedule>, EngineError> {
        let branch = self.repo_ref(pid)?.branch(bid)?;
        let key = (bid, branch.meta.version);
        if let Some((_, s)) = self.cache.iter().find(|(k, _)| *k == key) {
            return Ok(s.clone());
        }
        let snap = branch.head.snapshot();
        let project = branch.head.project.clone();
        let s = Arc::new(prswt_cpm::schedule(&prswt_cpm::ScheduleInput {
            project: &project,
            calendars: &snap.calendars,
            wbs: &snap.wbs,
            activities: &snap.activities,
            relationships: &snap.relationships,
        })?);
        self.cache.push((key, s.clone()));
        if self.cache.len() > SCHEDULE_CACHE {
            self.cache.remove(0);
        }
        Ok(s)
    }

    pub(crate) fn store_sim(&mut self, id: SimulationRunId, sim: StoredSim) {
        self.sims.push((id, sim));
        if self.sims.len() > SIM_CACHE {
            self.sims.remove(0);
        }
    }
}

#[cfg(test)]
mod tests;
