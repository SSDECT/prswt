//! Error and issue model shared by engine, MCP and API ([[04-MCP-Spec]] §0).

use serde::{Deserialize, Serialize};

/// Machine-readable failure category. Serialized SCREAMING_SNAKE_CASE so MCP
/// clients and the UI can branch on it without parsing messages.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    /// The referenced entity (project, branch, activity, …) does not exist.
    NotFound,
    /// Input is malformed or breaks a domain rule; fix the request and retry.
    Validation,
    /// A dependency loop (relationships, WBS parents or risk triggers).
    Cycle,
    /// Optimistic concurrency failed: the branch moved past `expected_version`.
    VersionConflict,
    /// A sandbox merge hit fields changed differently on both sides.
    MergeConflict,
    /// The risk cannot be realized now (wrong status or outside its proximity window).
    RiskNotRealizable,
    /// An agent tried to change the live branch; agents work in sandboxes.
    LiveWriteForbidden,
    /// The caller's role on this project does not allow it (viewer writing,
    /// non-owner closing or sharing).
    Forbidden,
    /// A size or range guard tripped (horizon, batch size, iterations).
    Limit,
    /// Unexpected failure inside the engine; state was restored.
    Internal,
}

/// A failed operation. `details` carries structured context (e.g. the cycle path).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize, thiserror::Error)]
#[error("{code:?}: {message}")]
pub struct EngineError {
    /// Failure category, for programmatic handling.
    pub code: ErrorCode,
    /// Human-readable explanation, safe to show to users and agents.
    pub message: String,
    /// Optional structured context (e.g. the cycle path or merge conflicts).
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub details: serde_json::Value,
}

impl EngineError {
    /// Error with a code and message and no details.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: serde_json::Value::Null,
        }
    }

    /// Attaches structured details (builder style).
    #[must_use]
    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = details;
        self
    }

    /// Shorthand for a [`ErrorCode::Validation`] error.
    pub fn validation(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Validation, message)
    }

    /// Shorthand for a [`ErrorCode::NotFound`] error naming what was missing.
    pub fn not_found(what: impl std::fmt::Display) -> Self {
        Self::new(ErrorCode::NotFound, format!("{what} not found"))
    }
}

/// How serious a non-fatal [`Issue`] is.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Informational note.
    Info,
    /// Something looks wrong but the result is still usable.
    Warning,
    /// The result is questionable; the user should act.
    Error,
}

/// A non-fatal finding attached to an otherwise successful result.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Issue {
    /// How serious the finding is.
    pub severity: Severity,
    /// Stable short code for the finding (e.g. `DCMA_LOGIC`).
    pub code: String,
    /// Human-readable explanation.
    pub message: String,
    /// Entity the finding is about, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_05_error_wire_format() {
        let e = EngineError::new(ErrorCode::Cycle, "cycle detected")
            .with_details(serde_json::json!({ "path": ["A", "B", "A"] }));
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["code"], "CYCLE");
        assert_eq!(v["details"]["path"][1], "B");
        assert_eq!(e.to_string(), "Cycle: cycle detected");
        let plain = serde_json::to_value(EngineError::not_found("activity A100")).unwrap();
        assert_eq!(plain["code"], "NOT_FOUND");
        assert!(plain.get("details").is_none());
    }
}
