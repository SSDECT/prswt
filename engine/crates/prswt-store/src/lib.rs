//! PostgreSQL event store (DB-01..05, ADR-0005).
//!
//! The graph lives in memory; the database keeps an append-only event log
//! per branch plus each branch's base state. Loading = replay. A snapshot of
//! the head is written every [`SNAPSHOT_EVERY`] versions and verified on load.
//!
//! The API is synchronous (an internal tokio runtime) so CLI/MCP hosts stay
//! simple. Connection settings come from `DATABASE_URL` (temporarily from
//! the gitignored `.env`; Bitwarden later — see CLAUDE.md).

mod auth;
mod sharing;

use prswt_core::ids::{BranchId, ProjectId, SimulationRunId};
use prswt_core::{EngineError, ErrorCode};
use prswt_state::{Branch, BranchMeta, Event, ProjectState, Repo};
use serde_json::Value;
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::{Row, Transaction};
use uuid::Uuid;

pub use auth::{TokenInfo, TokenOwner};
pub use sharing::{MAX_SETTINGS_BYTES, Member, ROLES};

/// Versions between head snapshots.
pub const SNAPSHOT_EVERY: u64 = 100;
const MAX_CONNECTIONS: u32 = 5;

/// Connection to the PostgreSQL event store.
#[derive(Debug)]
pub struct Store {
    /// `Option` only so `Drop` can shut it down without blocking (see below).
    rt: Option<tokio::runtime::Runtime>,
    pool: PgPool,
}

/// What a sync wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SyncStats {
    /// Branches upserted.
    pub branches: usize,
    /// Events appended.
    pub events: usize,
    /// Snapshots written.
    pub snapshots: usize,
}

pub(crate) fn db_err(e: impl std::fmt::Display) -> EngineError {
    EngineError::new(ErrorCode::Internal, format!("database: {e}"))
}

fn json_err(e: impl std::fmt::Display) -> EngineError {
    EngineError::new(ErrorCode::Internal, format!("stored data: {e}"))
}

impl Store {
    /// Connects and applies migrations.
    pub fn connect(url: &str) -> Result<Self, EngineError> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(db_err)?;
        let pool = rt.block_on(async {
            let pool = PgPoolOptions::new()
                .max_connections(MAX_CONNECTIONS)
                .connect(url)
                .await
                .map_err(db_err)?;
            sqlx::migrate!("./migrations")
                .run(&pool)
                .await
                .map_err(db_err)?;
            Ok::<_, EngineError>(pool)
        })?;
        Ok(Self { rt: Some(rt), pool })
    }

    /// Runs `f` to completion on the store's runtime.
    pub(crate) fn block<T>(
        &self,
        f: impl std::future::Future<Output = Result<T, EngineError>>,
    ) -> Result<T, EngineError> {
        self.rt
            .as_ref()
            .ok_or_else(|| EngineError::new(ErrorCode::Internal, "store is shut down"))?
            .block_on(f)
    }

    /// The connection pool.
    pub(crate) fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Connects using `DATABASE_URL`.
    pub fn from_env() -> Result<Self, EngineError> {
        let url = std::env::var("DATABASE_URL")
            .map_err(|_| EngineError::validation("DATABASE_URL is not set (see .env.example)"))?;
        Self::connect(&url)
    }

    /// DB-03/04: persists branch metadata, new events and due snapshots.
    /// Refuses when the database is ahead of memory (another writer).
    pub fn sync(&self, repo: &Repo) -> Result<SyncStats, EngineError> {
        self.block(async {
            let mut tx = self.pool.begin().await.map_err(db_err)?;
            let live_name = repo
                .branch(repo.live)
                .map(|b| b.head.project.name.clone())
                .unwrap_or_default();
            sqlx::query(
                "INSERT INTO projects (id, name, live_branch) VALUES ($1, $2, $3)
                 ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name, updated_at = now()",
            )
            .bind(repo.project.0)
            .bind(&live_name)
            .bind(repo.live.0)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
            let mut stats = SyncStats::default();
            for b in repo.branches() {
                stats.branches += 1;
                let (events, snaps) = sync_branch(&mut tx, repo.project, b).await?;
                stats.events += events;
                stats.snapshots += snaps;
            }
            tx.commit().await.map_err(db_err)?;
            Ok(stats)
        })
    }

    /// Ids of all stored projects.
    pub fn project_ids(&self) -> Result<Vec<ProjectId>, EngineError> {
        self.block(async {
            let rows = sqlx::query("SELECT id FROM projects ORDER BY name")
                .fetch_all(&self.pool)
                .await
                .map_err(db_err)?;
            rows.iter()
                .map(|r| r.try_get::<Uuid, _>("id").map(ProjectId).map_err(db_err))
                .collect()
        })
    }

    /// DB-04/05: rebuilds a repo by replaying every branch's events.
    pub fn load(&self, project: ProjectId) -> Result<Repo, EngineError> {
        self.block(async {
            let p = sqlx::query("SELECT live_branch FROM projects WHERE id = $1")
                .bind(project.0)
                .fetch_optional(&self.pool)
                .await
                .map_err(db_err)?
                .ok_or_else(|| EngineError::not_found(format!("stored project {project}")))?;
            let live = BranchId(p.try_get::<Uuid, _>("live_branch").map_err(db_err)?);
            let rows = sqlx::query("SELECT id, meta, base FROM branches WHERE project_id = $1")
                .bind(project.0)
                .fetch_all(&self.pool)
                .await
                .map_err(db_err)?;
            let mut repo = Repo::from_parts(project, live, Vec::new());
            for r in &rows {
                let id = BranchId(r.try_get::<Uuid, _>("id").map_err(db_err)?);
                let mut meta: BranchMeta =
                    serde_json::from_value(r.try_get::<Value, _>("meta").map_err(db_err)?).map_err(json_err)?;
                let base: ProjectState =
                    serde_json::from_value(r.try_get::<Value, _>("base").map_err(db_err)?).map_err(json_err)?;
                let stored_version = meta.version;
                meta.version = 0;
                repo.insert_branch(Branch::restored(meta, base.clone(), base));
                let events = sqlx::query("SELECT event FROM events WHERE branch_id = $1 ORDER BY version")
                    .bind(id.0)
                    .fetch_all(&self.pool)
                    .await
                    .map_err(db_err)?;
                for e in &events {
                    let ev: Event =
                        serde_json::from_value(e.try_get::<Value, _>("event").map_err(db_err)?).map_err(json_err)?;
                    repo.apply_loaded_event(ev)?;
                }
                let head_version = repo.branch(id)?.meta.version;
                if head_version != stored_version {
                    return Err(EngineError::new(
                        ErrorCode::Internal,
                        format!("branch {id}: {head_version} events stored, metadata says {stored_version}"),
                    ));
                }
                verify_snapshot(&self.pool, &repo, id).await?;
            }
            Ok(repo)
        })
    }

    /// Loads every stored project.
    pub fn load_all(&self) -> Result<Vec<Repo>, EngineError> {
        self.project_ids()?
            .into_iter()
            .map(|p| self.load(p))
            .collect()
    }

    /// Stores a simulation summary.
    pub fn save_simulation(
        &self,
        id: SimulationRunId,
        project: ProjectId,
        branch: BranchId,
        summary: &Value,
    ) -> Result<(), EngineError> {
        self.block(async {
            sqlx::query(
                "INSERT INTO simulation_runs (id, project_id, branch_id, summary) VALUES ($1, $2, $3, $4)
                 ON CONFLICT (id) DO NOTHING",
            )
            .bind(id.0)
            .bind(project.0)
            .bind(branch.0)
            .bind(summary)
            .execute(&self.pool)
            .await
            .map_err(db_err)?;
            Ok(())
        })
    }

    /// Deletes a project and everything stored for it.
    pub fn delete_project(&self, project: ProjectId) -> Result<(), EngineError> {
        self.block(async {
            let mut tx = self.pool.begin().await.map_err(db_err)?;
            sqlx::query("DELETE FROM projects WHERE id = $1")
                .bind(project.0)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            sqlx::query("DELETE FROM project_members WHERE project_id = $1")
                .bind(project.0)
                .execute(&mut *tx)
                .await
                .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
            Ok(())
        })
    }
}

impl Drop for Store {
    /// Shuts the private runtime down without blocking, so a `Store` may be
    /// dropped from async code (server shutdown, async tests) without a panic.
    fn drop(&mut self) {
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

async fn sync_branch(
    tx: &mut Transaction<'_, sqlx::Postgres>,
    project: ProjectId,
    b: &Branch,
) -> Result<(usize, usize), EngineError> {
    let meta = serde_json::to_value(&b.meta).map_err(json_err)?;
    let base = serde_json::to_value(&b.base).map_err(json_err)?;
    sqlx::query(
        "INSERT INTO branches (id, project_id, meta, base) VALUES ($1, $2, $3, $4)
         ON CONFLICT (id) DO UPDATE SET meta = EXCLUDED.meta, updated_at = now()",
    )
    .bind(b.meta.id.0)
    .bind(project.0)
    .bind(&meta)
    .bind(&base)
    .execute(&mut **tx)
    .await
    .map_err(db_err)?;
    let stored: i64 =
        sqlx::query("SELECT COALESCE(MAX(version), 0) AS v FROM events WHERE branch_id = $1")
            .bind(b.meta.id.0)
            .fetch_one(&mut **tx)
            .await
            .map_err(db_err)?
            .try_get("v")
            .map_err(db_err)?;
    let stored = u64::try_from(stored).unwrap_or(0);
    if stored > b.meta.version {
        return Err(EngineError::new(
            ErrorCode::VersionConflict,
            format!(
                "branch {} is at version {stored} in the database but {} in memory; reload first",
                b.meta.name, b.meta.version
            ),
        ));
    }
    // Same version number can still hide a different history: the stored
    // tip must be exactly our event at that version.
    if stored > 0 {
        let tip: Value =
            sqlx::query("SELECT event FROM events WHERE branch_id = $1 AND version = $2")
                .bind(b.meta.id.0)
                .bind(i64::try_from(stored).unwrap_or(i64::MAX))
                .fetch_one(&mut **tx)
                .await
                .map_err(db_err)?
                .try_get("event")
                .map_err(db_err)?;
        let ours = b
            .event(stored)
            .map(serde_json::to_value)
            .transpose()
            .map_err(json_err)?;
        if ours.as_ref() != Some(&tip) {
            return Err(EngineError::new(
                ErrorCode::VersionConflict,
                format!(
                    "branch {} diverged from the database at version {stored}; reload first",
                    b.meta.name
                ),
            ));
        }
    }
    let mut written = 0;
    for e in b.events.iter().filter(|e| e.version > stored) {
        sqlx::query(
            "INSERT INTO events (branch_id, version, project_id, event) VALUES ($1, $2, $3, $4)",
        )
        .bind(b.meta.id.0)
        .bind(i64::try_from(e.version).unwrap_or(i64::MAX))
        .bind(project.0)
        .bind(serde_json::to_value(e).map_err(json_err)?)
        .execute(&mut **tx)
        .await
        .map_err(db_err)?;
        written += 1;
    }
    let mut snaps = 0;
    if written > 0 && b.meta.version / SNAPSHOT_EVERY > stored / SNAPSHOT_EVERY {
        sqlx::query("INSERT INTO snapshots (branch_id, version, state) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
            .bind(b.meta.id.0)
            .bind(i64::try_from(b.meta.version).unwrap_or(i64::MAX))
            .bind(serde_json::to_value(&b.head).map_err(json_err)?)
            .execute(&mut **tx)
            .await
            .map_err(db_err)?;
        snaps = 1;
    }
    Ok((written, snaps))
}

/// The newest snapshot must equal the replayed state at its version.
async fn verify_snapshot(pool: &PgPool, repo: &Repo, id: BranchId) -> Result<(), EngineError> {
    let row = sqlx::query(
        "SELECT version, state FROM snapshots WHERE branch_id = $1 ORDER BY version DESC LIMIT 1",
    )
    .bind(id.0)
    .fetch_optional(pool)
    .await
    .map_err(db_err)?;
    let Some(row) = row else { return Ok(()) };
    let version = u64::try_from(row.try_get::<i64, _>("version").map_err(db_err)?).unwrap_or(0);
    let snap: ProjectState =
        serde_json::from_value(row.try_get::<Value, _>("state").map_err(db_err)?)
            .map_err(json_err)?;
    if repo.state_at(id, version)? != snap {
        return Err(EngineError::new(
            ErrorCode::Internal,
            format!("branch {id}: replay differs from snapshot at version {version}"),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
