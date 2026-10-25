//! DSK-01: the `.prswt` project file (ADR-0009).
//!
//! A file holds one project: every branch's metadata, base state and event
//! log, the same parts the PostgreSQL store keeps (ADR-0005). Opening replays
//! the events, so a file round-trips heads, sandboxes and history exactly.
//! Undo/redo stacks are session state and start empty after opening.
//!
//! Files are hostile input: [`Repo::from_file`] checks the format tag and
//! version, branch structure, event order and integrity of every base and
//! head before anything reaches the workspace. No IO happens here; callers
//! read and write the text.

use std::collections::BTreeSet;

use prswt_core::ids::{BranchId, ProjectId};
use prswt_core::{EngineError, ErrorCode};
use serde::{Deserialize, Serialize};

use crate::branch::{Branch, BranchKind, BranchMeta, Event, Repo};
use crate::state::ProjectState;

/// Value of [`ProjectFile::format`]; anything else is not a PRSWT project.
pub const FILE_FORMAT: &str = "prswt-project";
/// File layout version this build writes and reads.
pub const FILE_VERSION: u32 = 1;
/// Most branches one file may hold (live + sandboxes).
pub const MAX_FILE_BRANCHES: usize = 1_000;
/// Largest project file text accepted, in bytes (256 MiB).
pub const MAX_FILE_BYTES: usize = 256 * 1024 * 1024;

/// One project as stored on disk (JSON).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct ProjectFile {
    /// Always [`FILE_FORMAT`].
    pub format: String,
    /// Layout version, [`FILE_VERSION`] when written by this build.
    pub version: u32,
    /// Project id.
    pub project: ProjectId,
    /// The live branch's id.
    pub live: BranchId,
    /// Every branch, live first.
    pub branches: Vec<FileBranch>,
    /// SYNC-01: set when the project is a copy of a cloud project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync: Option<SyncLink>,
}

/// SYNC-01: how a local copy relates to its cloud project.
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
pub struct SyncLink {
    /// Server origin, e.g. `https://prswt.juz.ai`.
    pub server: String,
    /// Per branch, the last version both sides agree on. Local events after
    /// it are changes not yet sent.
    pub synced: std::collections::BTreeMap<BranchId, u64>,
    /// Per branch, the status and review request the server last reported;
    /// a local difference is a change still to send.
    #[serde(default)]
    pub remote: std::collections::BTreeMap<
        BranchId,
        (crate::branch::BranchStatus, Option<crate::branch::Proposal>),
    >,
}

/// One branch inside a [`ProjectFile`].
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct FileBranch {
    /// Metadata; `version` equals the number of events.
    pub meta: BranchMeta,
    /// State at fork (the initial state for live).
    pub base: ProjectState,
    /// Events since the fork, versions 1, 2, … in order.
    pub events: Vec<Event>,
}

impl Repo {
    /// The whole project as a file document, live branch first.
    pub fn to_file(&self) -> ProjectFile {
        let mut branches: Vec<FileBranch> = self
            .branches()
            .map(|b| FileBranch {
                meta: b.meta.clone(),
                base: b.base.clone(),
                events: b.events.clone(),
            })
            .collect();
        branches.sort_by_key(|b| b.meta.id != self.live);
        ProjectFile {
            format: FILE_FORMAT.to_owned(),
            version: FILE_VERSION,
            project: self.project,
            live: self.live,
            branches,
            sync: None,
        }
    }

    /// Rebuilds a project from a file document by replaying its events.
    ///
    /// Errors (`Validation`) on a foreign format or another layout version,
    /// no or too many branches, duplicate ids, a missing or non-live `live`
    /// branch, a sandbox whose parent is absent, a project id that differs
    /// from the states inside, events out of order or for another branch,
    /// and any base or head that fails the integrity check.
    pub fn from_file(file: ProjectFile) -> Result<Self, EngineError> {
        if file.format != FILE_FORMAT {
            return Err(EngineError::validation(format!(
                "not a PRSWT project file (format `{}`)",
                file.format
            )));
        }
        if file.version != FILE_VERSION {
            return Err(EngineError::validation(format!(
                "project file version {} is not supported; this build reads version {FILE_VERSION}",
                file.version
            )));
        }
        if file.branches.is_empty() || file.branches.len() > MAX_FILE_BRANCHES {
            return Err(EngineError::validation(format!(
                "a project file holds 1..={MAX_FILE_BRANCHES} branches, this one {}",
                file.branches.len()
            )));
        }
        let ids: BTreeSet<BranchId> = file.branches.iter().map(|b| b.meta.id).collect();
        if ids.len() != file.branches.len() {
            return Err(EngineError::validation(
                "duplicate branch id in project file",
            ));
        }
        let live = file
            .branches
            .iter()
            .find(|b| b.meta.id == file.live)
            .ok_or_else(|| EngineError::validation("project file has no live branch"))?;
        if live.meta.kind != BranchKind::Live
            || file
                .branches
                .iter()
                .filter(|b| b.meta.kind == BranchKind::Live)
                .count()
                != 1
        {
            return Err(EngineError::validation(
                "project file must have exactly one live branch",
            ));
        }
        let mut repo = Self::from_parts(file.project, file.live, Vec::new());
        for b in file.branches {
            restore_branch(&mut repo, &ids, file.project, b)?;
        }
        Ok(repo)
    }
}

/// Checks one branch, inserts it and replays its events into the head.
fn restore_branch(
    repo: &mut Repo,
    ids: &BTreeSet<BranchId>,
    project: ProjectId,
    b: FileBranch,
) -> Result<(), EngineError> {
    let id = b.meta.id;
    if b.meta.parent.is_some_and(|p| !ids.contains(&p)) {
        return Err(EngineError::validation(format!(
            "branch `{}` names a parent that is not in the file",
            b.meta.name
        )));
    }
    if b.base.project.id != project {
        return Err(EngineError::validation(format!(
            "branch `{}` belongs to another project",
            b.meta.name
        )));
    }
    b.base.check_integrity()?;
    let declared = b.meta.version;
    let mut meta = b.meta;
    meta.version = 0;
    repo.insert_branch(Branch::restored(meta, b.base.clone(), b.base));
    for e in b.events {
        if e.branch != id {
            return Err(EngineError::validation(
                "project file has an event filed under the wrong branch",
            ));
        }
        repo.apply_loaded_event(e).map_err(|e| {
            EngineError::validation(format!("project file event log is damaged: {}", e.message))
        })?;
    }
    let head = &repo.branch(id)?.head;
    if repo.branch(id)?.meta.version != declared {
        return Err(EngineError::new(
            ErrorCode::Validation,
            format!("branch {id}: metadata says version {declared}, events reach a different one"),
        ));
    }
    if head.project.id != project {
        return Err(EngineError::validation(
            "an event moved the project to another id",
        ));
    }
    head.check_integrity()
}
