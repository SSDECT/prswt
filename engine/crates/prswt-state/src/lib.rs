//! Versioned project state (Pillar III foundation): persistent maps,
//! commands → events, sandboxes with O(1) fork, field-level three-way merge,
//! bounded undo/redo. See `vault/04-Decisions/ADR-0003-persistent-state-events.md`.

mod branch;
mod command;
mod file;
mod state;

pub use branch::{
    Author, Branch, BranchKind, BranchMeta, BranchStatus, Change, Conflict, Event, EventKind,
    MergeStrategy, Proposal, Repo, UNDO_DEPTH, diff_states, rebase, sync_commands,
};
pub use command::{Command, MAX_BATCH};
pub use file::{
    FILE_FORMAT, FILE_VERSION, FileBranch, MAX_FILE_BRANCHES, MAX_FILE_BYTES, ProjectFile, SyncLink,
};
pub use state::{EntityKind, EntityRef, ProjectState, Snapshot};

#[cfg(test)]
mod tests;
