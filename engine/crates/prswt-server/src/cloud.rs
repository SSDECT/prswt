//! SYNC-01/03/04 (ADR-0010): shared cloud projects on the server.
//!
//! * Members: `GET/PUT /v1/projects/{id}/members`, `DELETE …/members/{email}`.
//!   Owners invite people by email (invited people may sign in even when
//!   their domain is not on the allowlist) and set roles.
//! * Live updates: `GET /v1/projects/{id}/watch` streams Server-Sent Events:
//!   `change` for every new version and `presence` with who is connected.
//! * The desktop app's sign-in (`/auth/desktop`): a loopback redirect with
//!   PKCE (RFC 8252) turns the browser session into a `desktop` token.
//! * `GET /v1/me`, `PUT /v1/me/settings`: the account and its settings.
//!
//! Project data itself moves through the engine's `sync.*` commands on
//! `/v1/commands`, where roles are enforced per command.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Redirect, Response};
use axum::{Form, Json};
use futures_util::Stream;
use prswt_app::Role;
use prswt_core::ids::ProjectId;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::BroadcastStream;

use crate::{AppState, Caller, TOKEN_PREFIX, oidc, pages, unix_now};

/// Seconds a desktop sign-in code stays valid.
const CODE_TTL_SECS: u64 = 120;
/// Most desktop sign-ins in progress at once.
const MAX_CODES: usize = 1_000;
/// Most people shown as present per project, and most watch streams open
/// on one project.
const MAX_PRESENT: usize = 200;
/// Most watch streams one account may hold open at once.
const MAX_STREAMS_PER_ACCOUNT: usize = 8;
/// Most watch streams open on the server at once.
const MAX_STREAMS: usize = 5_000;

/// Who has a project open right now (one entry per watch connection).
#[derive(Debug, Default)]
pub struct Presence {
    open: Mutex<HashMap<ProjectId, Vec<(u64, String)>>>,
    next: Mutex<u64>,
    /// `{project, people}` whenever someone arrives or leaves.
    pub changes: Option<broadcast::Sender<Value>>,
    /// `(project, email)` whose streams must end now (removed from the
    /// project).
    kicks: Option<broadcast::Sender<(ProjectId, String)>>,
}

impl Presence {
    /// A registry that announces changes on a fresh channel.
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        let (kicks, _) = broadcast::channel(64);
        Self {
            changes: Some(tx),
            kicks: Some(kicks),
            ..Self::default()
        }
    }

    /// Ends `email`'s watch streams on `project` (they were removed).
    pub fn kick(&self, project: ProjectId, email: &str) {
        if let Some(tx) = &self.kicks {
            let _ = tx.send((project, email.to_owned()));
        }
    }

    /// Resolves when `email` is removed from `project`.
    fn kicked(
        &self,
        project: ProjectId,
        email: String,
    ) -> impl std::future::Future<Output = ()> + Send + use<> {
        let rx = self.kicks.as_ref().map(broadcast::Sender::subscribe);
        async move {
            let Some(mut rx) = rx else {
                return std::future::pending().await;
            };
            loop {
                match rx.recv().await {
                    Ok((p, e)) if p == project && e == email => return,
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => {
                        return std::future::pending().await;
                    }
                }
            }
        }
    }

    /// Distinct emails present on `project`, sorted.
    pub fn people(&self, project: ProjectId) -> Vec<String> {
        let Ok(m) = self.open.lock() else {
            return Vec::new();
        };
        let mut people: Vec<String> = m
            .get(&project)
            .map(|v| v.iter().map(|e| e.1.clone()).collect())
            .unwrap_or_default();
        people.sort();
        people.dedup();
        people.truncate(MAX_PRESENT);
        people
    }

    /// Registers a watcher; it leaves when the returned guard is dropped.
    /// `None` when the project, the account or the server already has as
    /// many streams as allowed.
    pub(crate) fn join(self: &Arc<Self>, project: ProjectId, email: &str) -> Option<Leave> {
        let id = self.next.lock().map_or(0, |mut n| {
            *n = n.wrapping_add(1);
            *n
        });
        {
            let mut m = self.open.lock().ok()?;
            let total: usize = m.values().map(Vec::len).sum();
            let mine = m.values().flatten().filter(|e| e.1 == email).count();
            let here = m.get(&project).map_or(0, Vec::len);
            if total >= MAX_STREAMS || mine >= MAX_STREAMS_PER_ACCOUNT || here >= MAX_PRESENT {
                return None;
            }
            m.entry(project).or_default().push((id, email.to_owned()));
        }
        self.announce(project);
        Some(Leave {
            presence: self.clone(),
            project,
            id,
        })
    }

    fn announce(&self, project: ProjectId) {
        if let Some(tx) = &self.changes {
            let _ = tx.send(json!({ "project": project, "people": self.people(project) }));
        }
    }
}

/// Removes a watcher when its event stream is dropped.
pub(crate) struct Leave {
    presence: Arc<Presence>,
    project: ProjectId,
    id: u64,
}

impl Drop for Leave {
    fn drop(&mut self) {
        if let Ok(mut m) = self.presence.open.lock()
            && let Some(v) = m.get_mut(&self.project)
        {
            v.retain(|e| e.0 != self.id);
            if v.is_empty() {
                m.remove(&self.project);
            }
        }
        self.presence.announce(self.project);
    }
}

/// A desktop sign-in waiting for the app to fetch its token.
#[derive(Debug, Clone)]
pub struct DesktopCode {
    email: String,
    challenge: String,
    created: u64,
}

/// One-time codes of desktop sign-ins, keyed by the code's hash.
#[derive(Debug, Default)]
pub struct DesktopCodes(Mutex<HashMap<String, DesktopCode>>);

impl DesktopCodes {
    /// Remembers a code; drops expired ones and, when full, the oldest.
    fn insert(&self, hash: String, code: DesktopCode, now: u64) -> bool {
        let Ok(mut m) = self.0.lock() else {
            return false;
        };
        m.retain(|_, c| now.saturating_sub(c.created) < CODE_TTL_SECS);
        if m.len() >= MAX_CODES
            && let Some(oldest) = m
                .iter()
                .min_by_key(|(_, c)| c.created)
                .map(|(k, _)| k.clone())
        {
            m.remove(&oldest);
        }
        m.insert(hash, code);
        true
    }

    fn take(&self, hash: &str, now: u64) -> Option<DesktopCode> {
        let c = self.0.lock().ok()?.remove(hash)?;
        (now.saturating_sub(c.created) < CODE_TTL_SECS).then_some(c)
    }
}

// ───────────────────────────── members ─────────────────────────────

/// Role of the caller on `project`, if any.
fn role_of(caller: &Caller, project: ProjectId) -> Option<Role> {
    caller
        .ctx
        .access
        .as_ref()
        .and_then(|a| a.get(&project).copied())
}

/// `GET /v1/projects/{id}/members`: anyone on the project may look.
pub async fn members(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ProjectId>,
) -> Response {
    if role_of(&caller, id).is_none() {
        return not_found();
    }
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.members(id.0)).await {
        Ok(Ok(m)) => Json(json!({
            "project": id,
            "members": m,
            "present": state.presence.people(id),
        }))
        .into_response(),
        _ => internal(),
    }
}

/// Body of `PUT /v1/projects/{id}/members`.
#[derive(Debug, Deserialize)]
pub struct Invite {
    email: String,
    role: String,
}

/// `PUT /v1/projects/{id}/members`: owners add people or change roles.
pub async fn set_member(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ProjectId>,
    Json(body): Json<Invite>,
) -> Response {
    if caller.kind == "read" {
        return forbidden("a read-only token cannot share projects");
    }
    match role_of(&caller, id) {
        None => return not_found(),
        Some(Role::Owner) => {}
        Some(_) => return forbidden("only owners share a project"),
    }
    let email = body.email.trim().to_ascii_lowercase();
    if !valid_email(&email) {
        return bad("that is not an email address");
    }
    let (store, by, role) = (state.store.clone(), caller.email.clone(), body.role);
    match tokio::task::spawn_blocking(move || store.set_member(id.0, &email, &role, &by)).await {
        Ok(Ok(())) => Json(json!({ "ok": true })).into_response(),
        Ok(Err(e)) => bad(&e.message),
        Err(_) => internal(),
    }
}

/// `DELETE /v1/projects/{id}/members/{email}`: owners remove anyone; anyone
/// may leave. The last owner stays.
pub async fn remove_member(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path((id, email)): Path<(ProjectId, String)>,
) -> Response {
    let email = email.to_ascii_lowercase();
    if caller.kind == "read" {
        return forbidden("a read-only token cannot change members");
    }
    match role_of(&caller, id) {
        None => return not_found(),
        Some(Role::Owner) => {}
        Some(_) if email == caller.email => {}
        Some(_) => return forbidden("only owners remove people"),
    }
    let (store, gone) = (state.store.clone(), email.clone());
    match tokio::task::spawn_blocking(move || store.remove_member(id.0, &email)).await {
        Ok(Ok(removed)) => {
            // Their open live-update streams end now, not at reconnect.
            state.presence.kick(id, &gone);
            Json(json!({ "removed": removed })).into_response()
        }
        Ok(Err(e)) => bad(&e.message),
        Err(_) => internal(),
    }
}

// ───────────────────────────── live updates ─────────────────────────────

/// `GET /v1/projects/{id}/watch`: `change` events for the project's new
/// versions and `presence` events with who has it open (this caller
/// included while the stream lasts).
pub async fn watch(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<ProjectId>,
) -> Response {
    if role_of(&caller, id).is_none() {
        return not_found();
    }
    let key = id.to_string();
    let Some(leave) = state.presence.join(id, &caller.email) else {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({ "error": "too many live-update connections" })),
        )
            .into_response();
    };
    let kicked = state.presence.kicked(id, caller.email.clone());
    let first = Event::default()
        .event("presence")
        .data(json!({ "project": id, "people": state.presence.people(id) }).to_string());
    let k1 = key.clone();
    let changes = BroadcastStream::new(state.events.subscribe())
        .filter_map(Result::ok)
        .filter(move |v| v["project"].as_str() == Some(k1.as_str()))
        .map(|v| Event::default().event("change").data(v.to_string()));
    let people = match &state.presence.changes {
        Some(tx) => BroadcastStream::new(tx.subscribe()),
        None => BroadcastStream::new(broadcast::channel(1).0.subscribe()),
    }
    .filter_map(Result::ok)
    .filter(move |v| v["project"].as_str() == Some(key.as_str()))
    .map(|v| Event::default().event("presence").data(v.to_string()));
    let stream = tokio_stream::once(first)
        .chain(changes.merge(people))
        .map(move |e| {
            // The stream owns the presence entry until the client goes.
            let _ = &leave;
            Ok(e)
        });
    let stream: std::pin::Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>> = Box::pin(
        futures_util::StreamExt::take_until(stream, Box::pin(kicked)),
    );
    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

// ───────────────────────────── account ─────────────────────────────

/// `GET /v1/me`: who the token or session belongs to, plus saved settings.
pub async fn me(State(state): State<AppState>, Extension(caller): Extension<Caller>) -> Response {
    let (store, email) = (state.store.clone(), caller.email.clone());
    let settings = tokio::task::spawn_blocking(move || store.settings(&email))
        .await
        .ok()
        .and_then(Result::ok)
        .flatten()
        .unwrap_or_else(|| json!({}));
    Json(json!({
        "email": caller.email,
        "kind": caller.kind,
        "projects": caller.ctx.access.as_ref().map(|a| {
            a.iter()
                .map(|(p, r)| json!({ "project": p, "role": r }))
                .collect::<Vec<_>>()
        }),
        "settings": settings,
    }))
    .into_response()
}

/// `DELETE /v1/me/token`: a token revokes itself (the desktop app signing
/// out), so a copy left behind stops working too.
pub async fn revoke_own_token(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Response {
    let Some(id) = caller.token else {
        return bad("only a token can revoke itself; sign out of the browser instead");
    };
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.revoke_token(id, &caller.email)).await {
        Ok(Ok(revoked)) => Json(json!({ "revoked": revoked })).into_response(),
        Ok(Err(e)) => bad(&e.message),
        Err(_) => internal(),
    }
}

/// `PUT /v1/me/settings`: replaces the account's settings document.
pub async fn put_settings(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<Value>,
) -> Response {
    let store = state.store.clone();
    match tokio::task::spawn_blocking(move || store.put_settings(&caller.email, &body)).await {
        Ok(Ok(())) => Json(json!({ "ok": true })).into_response(),
        Ok(Err(e)) => bad(&e.message),
        Err(_) => internal(),
    }
}

// ───────────────────────────── desktop sign-in ─────────────────────────────

/// Query of `/auth/desktop`, sent by the desktop app through the browser.
#[derive(Debug, Deserialize)]
pub struct DesktopStart {
    port: u16,
    state: String,
    challenge: String,
}

impl DesktopStart {
    fn valid(&self) -> bool {
        self.port >= 1024
            && (8..=128).contains(&self.state.len())
            && self.challenge.len() == 43
            && self
                .state
                .chars()
                .chain(self.challenge.chars())
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    }
}

/// `GET /auth/desktop`: asks the signed-in person to let the desktop app in
/// (signing in first when needed).
pub async fn desktop_start(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Query(q): Query<DesktopStart>,
) -> Response {
    if !q.valid() {
        return pages::message("Sign-in failed", "The desktop app sent an invalid request.");
    }
    let Some(caller) = crate::identify(&state, &headers)
        .await
        .filter(|c| c.via_cookie)
    else {
        let next = format!(
            "/auth/desktop?port={}&state={}&challenge={}",
            q.port, q.state, q.challenge
        );
        return Redirect::to(&format!("/login?next={}", urlencode(&next))).into_response();
    };
    pages::desktop_consent(&caller.email, q.port, &q.state, &q.challenge)
}

/// `POST /auth/desktop`: the person allowed it; hand a one-time code to the
/// app's loopback listener.
pub async fn desktop_allow(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Form(q): Form<DesktopStart>,
) -> Response {
    if !caller.via_cookie || !q.valid() {
        return forbidden("sign in with the browser first");
    }
    let Some(code) = oidc::random_token(32) else {
        return internal();
    };
    let now = unix_now();
    if !state.desktop_codes.insert(
        oidc::hash_secret(&code),
        DesktopCode {
            email: caller.email.clone(),
            challenge: q.challenge.clone(),
            created: now,
        },
        now,
    ) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "too many sign-ins; try again",
        )
            .into_response();
    }
    Redirect::to(&format!(
        "http://127.0.0.1:{}/callback?code={code}&state={}",
        q.port, q.state
    ))
    .into_response()
}

/// Body of `POST /auth/desktop/token`.
#[derive(Debug, Deserialize)]
pub struct DesktopToken {
    code: String,
    verifier: String,
    #[serde(default)]
    device: String,
}

/// `POST /auth/desktop/token`: the app trades its code and PKCE verifier
/// for a `desktop` token (it acts as the person; revocable on /account).
pub async fn desktop_token(
    State(state): State<AppState>,
    Json(body): Json<DesktopToken>,
) -> Response {
    let Some(pending) = state
        .desktop_codes
        .take(&oidc::hash_secret(&body.code), unix_now())
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "unknown or expired code" })),
        )
            .into_response();
    };
    if oidc::pkce_challenge(&body.verifier) != pending.challenge {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "verifier does not match" })),
        )
            .into_response();
    }
    let Some(secret) = oidc::random_token(32).map(|s| format!("{TOKEN_PREFIX}{s}")) else {
        return internal();
    };
    let device: String = body
        .device
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || "-_. ".contains(*c))
        .take(30)
        .collect();
    let name = if device.trim().is_empty() {
        "PRSWT desktop".to_owned()
    } else {
        format!("PRSWT desktop · {}", device.trim())
    };
    let (store, hash, email) = (
        state.store.clone(),
        oidc::hash_secret(&secret),
        pending.email.clone(),
    );
    match tokio::task::spawn_blocking(move || {
        store.create_token(uuid::Uuid::now_v7(), &hash, &email, &name, "desktop")
    })
    .await
    {
        Ok(Ok(())) => Json(json!({ "token": secret, "email": pending.email })).into_response(),
        Ok(Err(e)) => bad(&e.message),
        Err(_) => internal(),
    }
}

// ───────────────────────────── helpers ─────────────────────────────

fn valid_email(e: &str) -> bool {
    e.len() <= 200
        && e.split_once('@').is_some_and(|(u, d)| {
            !u.is_empty() && d.contains('.') && !d.starts_with('.') && !d.ends_with('.')
        })
        && !e
            .chars()
            .any(|c| c.is_whitespace() || c == '<' || c == '>' || c == '"')
}

/// Percent-encodes a query value.
pub fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": "no such project" })),
    )
        .into_response()
}

fn forbidden(why: &str) -> Response {
    (StatusCode::FORBIDDEN, Json(json!({ "error": why }))).into_response()
}

fn bad(why: &str) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "error": why })),
    )
        .into_response()
}

fn internal() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": "server error" })),
    )
        .into_response()
}
