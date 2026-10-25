//! AUTH-02/03 persistence: login sessions and personal access tokens.
//!
//! Callers pass SHA-256 hashes of the secrets; plaintext secrets are never
//! stored. Expiry is computed by PostgreSQL (`now()`), so the engine still
//! never reads the clock.

use serde::Serialize;
use sqlx::Row;
use uuid::Uuid;

use crate::{Store, db_err};
use prswt_core::EngineError;

/// Most tokens one user may hold (hostile-input guard).
const MAX_TOKENS_PER_USER: i64 = 50;

/// A personal access token as listed to its owner (never the secret).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TokenInfo {
    /// Token id (used to revoke it).
    pub id: Uuid,
    /// Owner-chosen label, e.g. "claude-laptop".
    pub name: String,
    /// Creation time (UTC, ISO-8601).
    pub created_at: String,
    /// Last successful use, if any.
    pub last_used_at: Option<String>,
    /// Revocation time, if revoked.
    pub revoked_at: Option<String>,
    /// `agent`, `read` or `desktop`.
    pub kind: String,
}

/// Owner of a valid access token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenOwner {
    /// Token id.
    pub id: Uuid,
    /// Owner email.
    pub email: String,
    /// Token label (used as the agent name).
    pub name: String,
    /// `agent` (sandbox-only agent), `read` (read-only agent) or `desktop`
    /// (the desktop app acting as its person).
    pub kind: String,
}

impl Store {
    /// Records a session valid for `hours`.
    pub fn create_session(
        &self,
        token_hash: &str,
        email: &str,
        provider: &str,
        hours: i32,
    ) -> Result<(), EngineError> {
        self.block(async {
            sqlx::query(
                "INSERT INTO sessions (token_hash, email, provider, expires_at)
                 VALUES ($1, $2, $3, now() + make_interval(hours => $4))",
            )
            .bind(token_hash)
            .bind(email)
            .bind(provider)
            .bind(hours)
            .execute(self.pool())
            .await
            .map_err(db_err)?;
            Ok(())
        })
    }

    /// Email of an unexpired session.
    pub fn session_email(&self, token_hash: &str) -> Result<Option<String>, EngineError> {
        self.block(async {
            let row = sqlx::query(
                "SELECT email FROM sessions WHERE token_hash = $1 AND expires_at > now()",
            )
            .bind(token_hash)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
            row.map(|r| r.try_get::<String, _>("email").map_err(db_err))
                .transpose()
        })
    }

    /// Ends a session (logout).
    pub fn delete_session(&self, token_hash: &str) -> Result<(), EngineError> {
        self.block(async {
            sqlx::query("DELETE FROM sessions WHERE token_hash = $1 OR expires_at < now()")
                .bind(token_hash)
                .execute(self.pool())
                .await
                .map_err(db_err)?;
            Ok(())
        })
    }

    /// Stores a new token of `kind` (`agent`, `read`, `desktop`) for
    /// `email`; fails beyond 50 active tokens per user.
    pub fn create_token(
        &self,
        id: Uuid,
        token_hash: &str,
        email: &str,
        name: &str,
        kind: &str,
    ) -> Result<(), EngineError> {
        self.block(async {
            let active: i64 = sqlx::query(
                "SELECT COUNT(*) AS n FROM access_tokens WHERE email = $1 AND revoked_at IS NULL",
            )
            .bind(email)
            .fetch_one(self.pool())
            .await
            .map_err(db_err)?
            .try_get("n")
            .map_err(db_err)?;
            if active >= MAX_TOKENS_PER_USER {
                return Err(EngineError::new(
                    prswt_core::ErrorCode::Limit,
                    "too many active tokens; revoke some first",
                ));
            }
            sqlx::query(
                "INSERT INTO access_tokens (id, token_hash, email, name, kind) VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(id)
            .bind(token_hash)
            .bind(email)
            .bind(name)
            .bind(kind)
            .execute(self.pool())
            .await
            .map_err(db_err)?;
            Ok(())
        })
    }

    /// Tokens of `email`, newest first.
    pub fn list_tokens(&self, email: &str) -> Result<Vec<TokenInfo>, EngineError> {
        self.block(async {
            let rows = sqlx::query(
                "SELECT id, name, kind, created_at::text AS c, last_used_at::text AS u, revoked_at::text AS r
                 FROM access_tokens WHERE email = $1 ORDER BY created_at DESC",
            )
            .bind(email)
            .fetch_all(self.pool())
            .await
            .map_err(db_err)?;
            rows.iter()
                .map(|r| {
                    Ok(TokenInfo {
                        id: r.try_get("id").map_err(db_err)?,
                        name: r.try_get("name").map_err(db_err)?,
                        created_at: r.try_get("c").map_err(db_err)?,
                        last_used_at: r.try_get("u").map_err(db_err)?,
                        revoked_at: r.try_get("r").map_err(db_err)?,
                        kind: r.try_get("kind").map_err(db_err)?,
                    })
                })
                .collect()
        })
    }

    /// Revokes token `id` owned by `email`; `false` when no such active token.
    pub fn revoke_token(&self, id: Uuid, email: &str) -> Result<bool, EngineError> {
        self.block(async {
            let done = sqlx::query("UPDATE access_tokens SET revoked_at = now() WHERE id = $1 AND email = $2 AND revoked_at IS NULL")
                .bind(id)
                .bind(email)
                .execute(self.pool())
                .await
                .map_err(db_err)?;
            Ok(done.rows_affected() == 1)
        })
    }

    /// Owner of an active token, marking it used.
    pub fn use_token(&self, token_hash: &str) -> Result<Option<TokenOwner>, EngineError> {
        self.block(async {
            let row = sqlx::query(
                "UPDATE access_tokens SET last_used_at = now()
                 WHERE token_hash = $1 AND revoked_at IS NULL RETURNING id, email, name, kind",
            )
            .bind(token_hash)
            .fetch_optional(self.pool())
            .await
            .map_err(db_err)?;
            row.map(|r| {
                Ok(TokenOwner {
                    id: r.try_get("id").map_err(db_err)?,
                    email: r.try_get("email").map_err(db_err)?,
                    name: r.try_get("name").map_err(db_err)?,
                    kind: r.try_get("kind").map_err(db_err)?,
                })
            })
            .transpose()
        })
    }
}
