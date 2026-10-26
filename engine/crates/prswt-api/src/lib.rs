//! HTTP API (API-01/02): the command catalog over REST, plus a live event
//! stream.
//!
//! * `GET  /v1/commands`               — catalog (ids, labels, P6 menus, params)
//! * `POST /v1/commands/{id}`          — run a command; JSON body = params
//! * `GET  /v1/events`                 — Server-Sent Events, one per recorded change
//! * `GET  /v1/health`                 — liveness
//!
//! Like MCP and the CLI it is a thin adapter over [`Workspace::execute`].
//! It binds to the loopback addresses only (127.0.0.1 and ::1). With [`ApiState::token`] set (the CLI always
//! sets one) every route but `/v1/health` needs `Authorization: Bearer
//! <token>`, so other local programs and web pages cannot drive the engine.
//! Callers sending `X-PRSWT-Agent: <name>` act as agents and are subject to
//! the agent policy; others act as humans.

use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use futures_util::Stream;
use prswt_app::{Ctx, Workspace};
use prswt_core::{EngineError, ErrorCode};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

/// Buffered change notifications per subscriber before old ones are dropped.
const EVENT_BUFFER: usize = 256;

/// Called after every successful write (e.g. persist to PostgreSQL).
pub type AfterWrite = Arc<dyn Fn(&Workspace) -> Result<(), EngineError> + Send + Sync>;

/// Shared server state.
#[derive(Clone)]
pub struct ApiState {
    /// The engine, shared with other adapters (UI, control channel).
    pub workspace: Arc<Mutex<Workspace>>,
    /// Change notifications fanned out to `/v1/events` subscribers.
    pub events: broadcast::Sender<Value>,
    /// Persistence hook run after writes.
    pub after_write: Option<AfterWrite>,
    /// Trust the `X-PRSWT-Agent` header when no authenticated identity is
    /// attached (local development only; the public server sets `false`).
    pub header_identity: bool,
    /// Shared secret every request must present as a bearer token (local
    /// API); `None` leaves authentication to an outer layer (the server).
    pub token: Option<String>,
}

impl std::fmt::Debug for ApiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiState").finish_non_exhaustive()
    }
}

impl ApiState {
    /// Wraps `workspace` and installs the event hook that feeds `/v1/events`.
    pub fn new(workspace: Arc<Mutex<Workspace>>) -> Self {
        let (tx, _) = broadcast::channel(EVENT_BUFFER);
        if let Ok(mut ws) = workspace.lock() {
            let sender = tx.clone();
            ws.on_event = Some(Box::new(move |project, event| {
                // No subscribers is fine; the event is simply not delivered.
                let _ = sender.send(json!({
                    "project": project,
                    "branch": event.branch,
                    "version": event.version,
                    "author": event.author,
                    "changes": event.changes.len(),
                }));
            }));
        }
        Self {
            workspace,
            events: tx,
            after_write: None,
            header_identity: true,
            token: None,
        }
    }
}

/// The API routes.
pub fn router(state: ApiState) -> Router {
    Router::new()
        .route("/v1/commands", get(list_commands))
        .route("/v1/commands/{id}", post(run_command))
        .route("/v1/events", get(events))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_token))
        .route("/v1/health", get(|| async { Json(json!({ "ok": true })) }))
        .with_state(state)
}

/// Refuses requests without the bearer token when one is configured.
async fn require_token(State(state): State<ApiState>, req: Request, next: Next) -> Response {
    let Some(expected) = state.token.as_deref() else {
        return next.run(req).await;
    };
    let given = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or_default();
    if same_secret(given.as_bytes(), expected.as_bytes()) {
        next.run(req).await
    } else {
        let err = EngineError::new(ErrorCode::Forbidden, "missing or wrong API token");
        (StatusCode::UNAUTHORIZED, Json(json!({ "error": err }))).into_response()
    }
}

/// Compares secrets in time independent of where they differ.
fn same_secret(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// A fresh random token (128 bits from the OS, hex).
pub fn new_token() -> Result<String, EngineError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|e| EngineError::new(ErrorCode::Internal, format!("no OS randomness: {e}")))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Serves on `127.0.0.1:port`, and on `[::1]:port` when the machine has an
/// IPv6 loopback, until the process ends. Both matter: `localhost` resolves
/// to `::1` first, and a client that finds nothing there waits about two
/// seconds per request before it falls back to IPv4 (seen on Windows).
pub async fn serve(state: ApiState, port: u16) -> std::io::Result<()> {
    use std::future::IntoFuture;
    let v4 = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let port = v4.local_addr()?.port();
    let app = router(state);
    match tokio::net::TcpListener::bind(("::1", port)).await {
        Ok(v6) => {
            tokio::try_join!(
                axum::serve(v4, app.clone()).into_future(),
                axum::serve(v6, app).into_future()
            )?;
            Ok(())
        }
        // No IPv6 loopback (or the port is taken there): IPv4 only.
        Err(_) => axum::serve(v4, app).await,
    }
}

async fn list_commands() -> Json<Value> {
    Json(json!({
        "commands": prswt_app::catalog().iter().map(|c| json!({
            "id": c.id, "label": c.label, "menu": c.menu, "params": c.params, "writes": c.writes,
        })).collect::<Vec<_>>()
    }))
}

async fn run_command(
    State(state): State<ApiState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    identity: Option<Extension<Ctx>>,
    body: Bytes,
) -> Response {
    // An authentication layer attaches the caller; without one (local use)
    // the optional header decides, otherwise the request is refused.
    let ctx = match (identity, state.header_identity) {
        (Some(Extension(ctx)), _) => ctx,
        (None, true) => match headers.get("x-prswt-agent").and_then(|v| v.to_str().ok()) {
            Some(name) => Ctx::agent(&name.chars().take(64).collect::<String>()),
            None => Ctx::human("http"),
        },
        (None, false) => {
            let err = EngineError::new(ErrorCode::LiveWriteForbidden, "authentication required");
            return (StatusCode::UNAUTHORIZED, Json(json!({ "error": err }))).into_response();
        }
    };
    // An empty body means "no params"; anything else must be JSON.
    let params = if body.iter().all(u8::is_ascii_whitespace) {
        Value::Null
    } else {
        match serde_json::from_slice(&body) {
            Ok(v) => v,
            Err(e) => {
                let err = EngineError::validation(format!("request body is not JSON: {e}"));
                return (StatusCode::BAD_REQUEST, Json(json!({ "error": err }))).into_response();
            }
        }
    };
    let ws = state.workspace.clone();
    let after = state.after_write.clone();
    // The engine is synchronous; keep it off the async executor threads.
    let result = tokio::task::spawn_blocking(move || match ws.lock() {
        Ok(mut w) => {
            let out = w.execute(&ctx, &id, &params)?;
            let writes = prswt_app::catalog().iter().any(|c| c.id == id && c.writes);
            if writes && let Some(hook) = &after {
                hook(&w)?;
            }
            Ok(out)
        }
        Err(_) => Err(EngineError::new(
            ErrorCode::Internal,
            "workspace unavailable",
        )),
    })
    .await
    .unwrap_or_else(|_| Err(EngineError::new(ErrorCode::Internal, "command task failed")));
    match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => (status_of(e.code), Json(json!({ "error": e }))).into_response(),
    }
}

/// HTTP status for an engine error code.
fn status_of(code: ErrorCode) -> StatusCode {
    match code {
        ErrorCode::NotFound => StatusCode::NOT_FOUND,
        ErrorCode::Validation | ErrorCode::Cycle | ErrorCode::RiskNotRealizable => {
            StatusCode::UNPROCESSABLE_ENTITY
        }
        ErrorCode::VersionConflict | ErrorCode::MergeConflict => StatusCode::CONFLICT,
        ErrorCode::LiveWriteForbidden | ErrorCode::Forbidden => StatusCode::FORBIDDEN,
        ErrorCode::Limit => StatusCode::PAYLOAD_TOO_LARGE,
        ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

async fn events(
    State(state): State<ApiState>,
    identity: Option<Extension<Ctx>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    // A scoped caller (shared server) only hears about its own projects.
    let visible: Option<std::collections::HashSet<String>> = identity
        .and_then(|Extension(c)| c.access)
        .map(|a| a.keys().map(ToString::to_string).collect());
    let stream = BroadcastStream::new(state.events.subscribe())
        .filter_map(Result::ok)
        .filter(move |v| {
            visible
                .as_ref()
                .is_none_or(|set| v["project"].as_str().is_some_and(|p| set.contains(p)))
        })
        .map(|v| Ok(Event::default().event("change").data(v.to_string())));
    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[cfg(test)]
mod tests;
