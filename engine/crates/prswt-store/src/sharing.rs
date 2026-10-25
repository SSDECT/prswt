//! SYNC-01/04 persistence (ADR-0010): project members and their roles, and
//! per-user settings. Roles are stored as text (`viewer`, `editor`,
//! `owner`); the server turns them into the engine's access map.

use serde::Serialize;
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

use crate::{Store, db_err};
use prswt_core::{EngineError, ErrorCode};

/// Largest settings document kept per user (bytes of JSON).
pub const MAX_SETTINGS_BYTES: usize = 64 * 1024;
/// Most members one project may have (hostile-input guard).
const MAX_MEMBERS: i64 = 500;
/// Role names accepted in the table.
pub const ROLES: [&str; 3] = ["viewer", "editor", "owner"];

/// One person's access to a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Member {
    /// Account email (lower case).
    pub email: String,
    /// `viewer`, `editor` or `owner`.
    pub role: String,
    /// Who granted it.
    pub added_by: String,
    /// When (UTC, ISO-8601).
    pub added_at: String,
}

impl Store {
    /// Every project `email` may use, with the role: `(project id, role)`.
    pub fn access_for(&self, email: &str) -> Result<Vec<(Uuid, String)>, EngineError> {
        self.block(async {
            let rows = sqlx::query("SELECT project_id, role FROM project_members WHERE email = $1")
                .bind(email)
                .fetch_all(self.pool())
                .await
                .map_err(db_err)?;
            rows.iter()
                .map(|r| {
                    Ok((
                        r.try_get("project_id").map_err(db_err)?,
                        r.try_get("role").map_err(db_err)?,
                    ))
                })
                .collect()
        })
    }

    /// Whether `email` belongs to any project (invited people may sign in).
    pub fn is_member_anywhere(&self, email: &str) -> Result<bool, EngineError> {
        self.block(async {
            let n: i64 = sqlx::query("SELECT COUNT(*) AS n FROM project_members WHERE email = $1")
                .bind(email)
                .fetch_one(self.pool())
                .await
                .map_err(db_err)?
                .try_get("n")
                .map_err(db_err)?;
            Ok(n > 0)
        })
    }

    /// Members of a project, owners first.
    pub fn members(&self, project: Uuid) -> Result<Vec<Member>, EngineError> {
        self.block(async {
            let rows = sqlx::query(
                "SELECT email, role, added_by, added_at::text AS a FROM project_members
                 WHERE project_id = $1
                 ORDER BY CASE role WHEN 'owner' THEN 0 WHEN 'editor' THEN 1 ELSE 2 END, email",
            )
            .bind(project)
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
            rows.iter()
                .map(|r| {
                    Ok(Member {
                        email: r.try_get("email").map_err(db_err)?,
                        role: r.try_get("role").map_err(db_err)?,
                        added_by: r.try_get("added_by").map_err(db_err)?,
                        added_at: r.try_get("a").map_err(db_err)?,
                    })
                })
                .collect()
        })
    }

    /// Adds `email` to a project or changes its role. Refuses an unknown
    /// role, too many members, and demoting the last owner.
    pub fn set_member(
        &self,
        project: Uuid,
        email: &str,
        role: &str,
        by: &str,
    ) -> Result<(), EngineError> {
        if !ROLES.contains(&role) {
            return Err(EngineError::validation(format!("unknown role `{role}`")));
        }
        self.block(async {
            let mut tx = self.pool().begin().await.map_err(db_err)?;
            let count: i64 =
                sqlx::query("SELECT COUNT(*) AS n FROM project_members WHERE project_id = $1")
                    .bind(project)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(db_err)?
                    .try_get("n")
                    .map_err(db_err)?;
            if count >= MAX_MEMBERS {
                return Err(EngineError::new(ErrorCode::Limit, "too many members"));
            }
            sqlx::query(
                "INSERT INTO project_members (project_id, email, role, added_by) VALUES ($1, $2, $3, $4)
                 ON CONFLICT (project_id, email) DO UPDATE SET role = EXCLUDED.role",
            )
            .bind(project)
            .bind(email)
            .bind(role)
            .bind(by)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
            ensure_an_owner(&mut tx, project).await?;
            tx.commit().await.map_err(db_err)?;
            Ok(())
        })
    }

    /// Makes `email` the owner of a project just created on the server.
    /// Refuses when the id is already taken (stored, or with members): a
    /// closed or someone else's project is never handed to a new owner.
    pub fn claim_project(&self, project: Uuid, email: &str) -> Result<(), EngineError> {
        self.block(async {
            let mut tx = self.pool().begin().await.map_err(db_err)?;
            let taken: bool = sqlx::query(
                "SELECT EXISTS (SELECT 1 FROM projects WHERE id = $1)
                     OR EXISTS (SELECT 1 FROM project_members WHERE project_id = $1) AS t",
            )
            .bind(project)
            .fetch_one(&mut *tx)
            .await
            .map_err(db_err)?
            .try_get("t")
            .map_err(db_err)?;
            if taken {
                return Err(EngineError::validation(
                    "a project with this id already exists on this server: ask its owner to share it",
                ));
            }
            sqlx::query(
                "INSERT INTO project_members (project_id, email, role, added_by)
                 VALUES ($1, $2, 'owner', $2)",
            )
            .bind(project)
            .bind(email)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;
            tx.commit().await.map_err(db_err)?;
            Ok(())
        })
    }

    /// Removes `email` from a project; `false` when it was not a member.
    /// The last owner cannot be removed.
    pub fn remove_member(&self, project: Uuid, email: &str) -> Result<bool, EngineError> {
        self.block(async {
            let mut tx = self.pool().begin().await.map_err(db_err)?;
            let done =
                sqlx::query("DELETE FROM project_members WHERE project_id = $1 AND email = $2")
                    .bind(project)
                    .bind(email)
                    .execute(&mut *tx)
                    .await
                    .map_err(db_err)?;
            ensure_an_owner(&mut tx, project).await?;
            tx.commit().await.map_err(db_err)?;
            Ok(done.rows_affected() == 1)
        })
    }

    /// Projects stored before sharing existed have no members: make each
    /// of `owners` an owner of them. Runs once per database (a flag in
    /// `server_flags`), so a project that later ends up without members is
    /// never handed to the listed people on a restart. Returns how many
    /// projects were adopted.
    pub fn adopt_orphans(&self, owners: &[String]) -> Result<usize, EngineError> {
        self.block(async {
            let mut tx = self.pool().begin().await.map_err(db_err)?;
            let first = sqlx::query(
                "INSERT INTO server_flags (name) VALUES ('adopt_orphans') ON CONFLICT DO NOTHING",
            )
            .execute(&mut *tx)
            .await
            .map_err(db_err)?
            .rows_affected()
                == 1;
            if !first {
                return Ok(0);
            }
            let rows = sqlx::query(
                "SELECT p.id FROM projects p
                 WHERE NOT EXISTS (SELECT 1 FROM project_members m WHERE m.project_id = p.id)",
            )
            .fetch_all(&mut *tx)
            .await
            .map_err(db_err)?;
            let orphans: Vec<Uuid> = rows
                .iter()
                .map(|r| r.try_get("id").map_err(db_err))
                .collect::<Result<_, _>>()?;
            for id in &orphans {
                for email in owners {
                    sqlx::query(
                        "INSERT INTO project_members (project_id, email, role, added_by)
                         VALUES ($1, $2, 'owner', 'migration') ON CONFLICT DO NOTHING",
                    )
                    .bind(id)
                    .bind(email)
                    .execute(&mut *tx)
                    .await
                    .map_err(db_err)?;
                }
            }
            tx.commit().await.map_err(db_err)?;
            Ok(if owners.is_empty() { 0 } else { orphans.len() })
        })
    }

    /// The settings document of `email`, if saved.
    pub fn settings(&self, email: &str) -> Result<Option<Value>, EngineError> {
        self.block(async {
            let row = sqlx::query("SELECT settings FROM user_settings WHERE email = $1")
                .bind(email)
                .fetch_optional(self.pool())
                .await
                .map_err(db_err)?;
            row.map(|r| r.try_get::<Value, _>("settings").map_err(db_err))
                .transpose()
        })
    }

    /// Replaces the settings document of `email` (a JSON object up to
    /// [`MAX_SETTINGS_BYTES`]).
    pub fn put_settings(&self, email: &str, settings: &Value) -> Result<(), EngineError> {
        if !settings.is_object() {
            return Err(EngineError::validation("settings must be a JSON object"));
        }
        if settings.to_string().len() > MAX_SETTINGS_BYTES {
            return Err(EngineError::new(ErrorCode::Limit, "settings are too large"));
        }
        self.block(async {
            sqlx::query(
                "INSERT INTO user_settings (email, settings) VALUES ($1, $2)
                 ON CONFLICT (email) DO UPDATE SET settings = EXCLUDED.settings, updated_at = now()",
            )
            .bind(email)
            .bind(settings)
            .execute(self.pool())
            .await
            .map_err(db_err)?;
            Ok(())
        })
    }
}

/// Rolls the transaction back (by erroring) when a project would be left
/// without an owner (also when its last member, the owner, leaves: a
/// project nobody owns could not be shared or managed again).
async fn ensure_an_owner(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    project: Uuid,
) -> Result<(), EngineError> {
    let o: i64 = sqlx::query(
        "SELECT COUNT(*) AS o FROM project_members WHERE project_id = $1 AND role = 'owner'",
    )
    .bind(project)
    .fetch_one(&mut **tx)
    .await
    .map_err(db_err)?
    .try_get("o")
    .map_err(db_err)?;
    if o == 0 {
        return Err(EngineError::validation(
            "a project needs at least one owner",
        ));
    }
    Ok(())
}
