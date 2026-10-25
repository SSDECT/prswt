//! `prswt-server` (SRV-01): the public PRSWT server behind `prswt.juz.ai`.
//!
//! One axum app serves OIDC sign-in, the account page for personal access
//! tokens, the HTTP API (`/v1`) and remote MCP (`/mcp`, streamable HTTP).
//! There is no web UI: people use the desktop app (ADR-0009). Humans authenticate with a session cookie; agents with
//! `Authorization: Bearer prswt_…` tokens and keep the sandbox policy.
//! Projects are shared per project (ADR-0010, [`cloud`]): every caller sees
//! only the projects it is a member of, with its role enforced per command.
//! Cloudflare's tunnel reaches it over HTTPS and verifies its certificate.
//! Configuration is read from the environment (see [`Config::from_env`]).
#![allow(clippy::print_stderr)]

mod cloud;
mod oidc;
mod pages;

use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Extension, Form, Router};
use prswt_app::{Access, Clock, Ctx, Role, Workspace};
use prswt_core::Instant;
use prswt_store::Store;
use serde::Deserialize;

use crate::oidc::{Pending, PendingLogins, Provider};

/// Session cookie name.
const COOKIE: &str = "prswt_session";
/// Prefix of personal access tokens (makes leaked tokens recognisable).
const TOKEN_PREFIX: &str = "prswt_";

/// Server settings from the environment.
#[derive(Debug, Clone)]
pub struct Config {
    /// Public origin, e.g. `https://prswt.juz.ai` (no trailing slash).
    pub public_url: String,
    /// Listen address (default `0.0.0.0:8080` inside the container).
    pub bind: SocketAddr,
    /// Lower-cased emails allowed to sign in.
    pub allowed: HashSet<String>,
    /// Lower-cased email domains allowed to sign in (`*` = any). People
    /// invited to a project may sign in regardless.
    pub allowed_domains: HashSet<String>,
    /// Configured identity providers.
    pub providers: Vec<Provider>,
    /// Session lifetime in hours.
    pub session_hours: i32,
    /// Local development sign-in as this email (only for http://localhost origins).
    pub dev_login: Option<String>,
    /// PEM certificate and key paths; `None` serves plain HTTP.
    pub tls: Option<(PathBuf, PathBuf)>,
}

impl Config {
    /// Reads `PRSWT_PUBLIC_URL`, `PRSWT_BIND`, `PRSWT_ALLOWED_EMAILS`,
    /// `PRSWT_ALLOWED_DOMAINS`,
    /// `GOOGLE_CLIENT_ID/SECRET`, `MICROSOFT_CLIENT_ID/SECRET/TENANT`,
    /// `MICROSOFT_ALLOWED_TENANTS` (with a multi-tenant `MICROSOFT_TENANT`),
    /// `PRSWT_SESSION_HOURS`, `PRSWT_DEV_LOGIN`,
    /// `PRSWT_TLS_CERT` + `PRSWT_TLS_KEY` (PEM paths; both or neither).
    pub fn from_env() -> Result<Self, String> {
        let var = |k: &str| {
            std::env::var(k)
                .ok()
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let public_url = var("PRSWT_PUBLIC_URL")
            .ok_or("PRSWT_PUBLIC_URL is required")?
            .trim_end_matches('/')
            .to_owned();
        let local = public_url.starts_with("http://localhost")
            || public_url.starts_with("http://127.0.0.1");
        if !public_url.starts_with("https://") && !local {
            return Err("PRSWT_PUBLIC_URL must be https:// (http only for localhost)".into());
        }
        let mut providers = Vec::new();
        if let (Some(id), Some(secret)) = (var("GOOGLE_CLIENT_ID"), var("GOOGLE_CLIENT_SECRET")) {
            providers.push(Provider::google(id, secret));
        }
        if let (Some(id), Some(secret)) =
            (var("MICROSOFT_CLIENT_ID"), var("MICROSOFT_CLIENT_SECRET"))
        {
            // No default: `common` without a tenant allowlist would let any
            // directory sign in as any email.
            let tenant = var("MICROSOFT_TENANT").ok_or(
                "set MICROSOFT_TENANT to your tenant id (or `organizations` with MICROSOFT_ALLOWED_TENANTS)",
            )?;
            let allowed: Vec<String> = var("MICROSOFT_ALLOWED_TENANTS")
                .unwrap_or_default()
                .split(',')
                .map(str::to_owned)
                .collect();
            providers.push(Provider::microsoft(id, secret, &tenant, &allowed)?);
        }
        let dev_login = var("PRSWT_DEV_LOGIN").filter(|_| local);
        if providers.is_empty() && dev_login.is_none() {
            return Err(
                "configure GOOGLE_CLIENT_ID/SECRET and/or MICROSOFT_CLIENT_ID/SECRET".into(),
            );
        }
        let allowed: HashSet<String> = var("PRSWT_ALLOWED_EMAILS")
            .unwrap_or_default()
            .split(',')
            .map(|e| e.trim().to_ascii_lowercase())
            .filter(|e| e.contains('@'))
            .collect();
        let allowed_domains: HashSet<String> = var("PRSWT_ALLOWED_DOMAINS")
            .unwrap_or_default()
            .split(',')
            .map(|d| d.trim().trim_start_matches('@').to_ascii_lowercase())
            .filter(|d| d == "*" || d.contains('.'))
            .collect();
        if allowed.is_empty() && allowed_domains.is_empty() {
            return Err(
                "set PRSWT_ALLOWED_EMAILS and/or PRSWT_ALLOWED_DOMAINS (at least one entry)".into(),
            );
        }
        Ok(Self {
            public_url,
            bind: var("PRSWT_BIND")
                .unwrap_or_else(|| "0.0.0.0:8080".into())
                .parse()
                .map_err(|_| "PRSWT_BIND is not an address")?,
            allowed,
            allowed_domains,
            providers,
            session_hours: var("PRSWT_SESSION_HOURS")
                .and_then(|h| h.parse().ok())
                .unwrap_or(168)
                .clamp(1, 24 * 30),
            dev_login,
            tls: match (var("PRSWT_TLS_CERT"), var("PRSWT_TLS_KEY")) {
                (Some(c), Some(k)) => Some((PathBuf::from(c), PathBuf::from(k))),
                (None, None) => None,
                _ => return Err("set both PRSWT_TLS_CERT and PRSWT_TLS_KEY, or neither".into()),
            },
        })
    }

    /// Whether `email` may sign in by configuration (listed, or its domain).
    pub fn allows(&self, email: &str) -> bool {
        self.allowed.contains(email)
            || self.allowed_domains.contains("*")
            || email
                .rsplit_once('@')
                .is_some_and(|(_, d)| self.allowed_domains.contains(d))
    }

    fn secure_cookies(&self) -> bool {
        self.public_url.starts_with("https://")
    }
}

/// Wall clock injected into the engine (UTC minutes since 2000-01-01).
struct SystemClock;

impl Clock for SystemClock {
    #[allow(clippy::disallowed_methods)]
    fn now(&self) -> Instant {
        Instant(
            i64::try_from(unix_now())
                .unwrap_or(0)
                .saturating_sub(946_684_800)
                / 60,
        )
    }
}

/// Unix seconds (the server host may read the clock; the engine may not).
#[allow(clippy::disallowed_methods)]
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Content-Security-Policy for every response. The pages are server-rendered
/// HTML with inline CSS and no scripts, so nothing else may load.
/// `form-action` also allows the desktop app's loopback listener, where the
/// desktop sign-in form redirects (RFC 8252).
pub const CSP: &str = "default-src 'none'; style-src 'unsafe-inline'; img-src 'self' data:; \
                       form-action 'self' http://127.0.0.1:*; base-uri 'none'; frame-ancestors 'none'";

/// Shared server state.
#[derive(Clone)]
pub struct AppState {
    cfg: Arc<Config>,
    store: Arc<Store>,
    pending: Arc<PendingLogins>,
    http: reqwest::Client,
    mcp: prswt_mcp::Shared,
    /// Change notifications of the engine (shared with `/v1/events`).
    events: tokio::sync::broadcast::Sender<serde_json::Value>,
    /// Who has which project open (watch streams).
    presence: Arc<cloud::Presence>,
    /// Desktop sign-ins waiting for their token.
    desktop_codes: Arc<cloud::DesktopCodes>,
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("public_url", &self.cfg.public_url)
            .finish_non_exhaustive()
    }
}

/// Who is calling, resolved from a bearer token or the session cookie.
#[derive(Debug, Clone)]
pub struct Caller {
    /// Engine identity (human for sessions, agent for tokens).
    pub ctx: Ctx,
    /// Account email.
    pub email: String,
    /// Authenticated by cookie (browser) rather than token.
    pub via_cookie: bool,
    /// `session`, or the token kind: `agent`, `read`, `desktop`.
    pub kind: String,
    /// The token's id (token callers only; a desktop signs out by revoking it).
    pub token: Option<uuid::Uuid>,
}

fn main() -> std::process::ExitCode {
    // `prswt-server healthcheck`: for the container HEALTHCHECK (no curl in the image).
    if std::env::args().nth(1).as_deref() == Some("healthcheck") {
        return healthcheck();
    }
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("prswt-server: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

/// Exit 0 when `GET /healthz` on the local listener answers 200. Uses
/// https when `PRSWT_TLS_CERT` is set; the certificate is not checked
/// because this only talks to its own container over loopback.
fn healthcheck() -> std::process::ExitCode {
    let port = std::env::var("PRSWT_BIND")
        .ok()
        .and_then(|b| b.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()))
        .unwrap_or(8080);
    let scheme = if std::env::var_os("PRSWT_TLS_CERT").is_some() {
        "https"
    } else {
        "http"
    };
    let ok = tokio::runtime::Runtime::new().is_ok_and(|rt| {
        rt.block_on(async {
            let client = reqwest::Client::builder()
                .tls_danger_accept_invalid_certs(true)
                .timeout(std::time::Duration::from_secs(4))
                .build();
            match client {
                Ok(c) => c
                    .get(format!("{scheme}://127.0.0.1:{port}/healthz"))
                    .send()
                    .await
                    .is_ok_and(|r| r.status().is_success()),
                Err(_) => false,
            }
        })
    });
    if ok {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

/// Makes the creator of a project its owner. A failure (the id is taken,
/// the database is down) fails the command, so no project is left without
/// an owner and nobody takes over an existing one.
pub(crate) fn owner_hook(store: Arc<Store>) -> prswt_app::OnCreate {
    Box::new(move |ctx: &Ctx, pid| match &ctx.account {
        Some(email) => store.claim_project(pid.0, email),
        None => Err(prswt_core::EngineError::new(
            prswt_core::ErrorCode::Forbidden,
            "only a signed-in account can create projects here",
        )),
    })
}

/// The persistence hook: saves every project after a write. Only a project
/// this request changed (it emitted events) can fail the request; another
/// project's error is logged, so one broken project does not block
/// everyone's writes.
pub(crate) fn persist_after_write(
    workspace: &Arc<Mutex<Workspace>>,
    store: Arc<Store>,
) -> prswt_api::AfterWrite {
    let changed: Arc<Mutex<std::collections::BTreeSet<prswt_core::ids::ProjectId>>> =
        Arc::default();
    if let Ok(mut w) = workspace.lock() {
        // Chained onto the API's hook (it feeds `/v1/events`).
        let mut inner = w.on_event.take();
        let seen = changed.clone();
        w.on_event = Some(Box::new(move |pid, event| {
            if let Ok(mut s) = seen.lock() {
                s.insert(pid);
            }
            if let Some(f) = inner.as_mut() {
                f(pid, event);
            }
        }));
    }
    Arc::new(move |w: &Workspace| {
        let mine = changed
            .lock()
            .map(|mut s| std::mem::take(&mut *s))
            .unwrap_or_default();
        let mut first = None;
        for repo in w.projects() {
            if let Err(e) = store.sync(repo) {
                if mine.contains(&repo.project) {
                    first.get_or_insert(e);
                } else {
                    eprintln!(
                        "prswt-server: could not save project {}: {}",
                        repo.project, e.message
                    );
                }
            }
        }
        first.map_or(Ok(()), Err)
    })
}

fn run() -> Result<(), String> {
    let cfg = Config::from_env()?;
    let url = std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is required")?;
    let store = Arc::new(Store::connect(&url).map_err(|e| e.message)?);
    let mut ws = Workspace::new(Arc::new(SystemClock));
    for repo in store.load_all().map_err(|e| e.message)? {
        ws.insert_repo(repo);
    }
    // Projects from before sharing belong to the listed people.
    let mut first: Vec<String> = cfg.allowed.iter().cloned().collect();
    first.sort();
    let adopted = store.adopt_orphans(&first).map_err(|e| e.message)?;
    if adopted > 0 {
        eprintln!("prswt-server: {adopted} existing project(s) shared with the listed accounts");
    }
    ws.on_create = Some(owner_hook(store.clone()));
    let workspace = Arc::new(Mutex::new(ws));
    let mut api = prswt_api::ApiState::new(workspace.clone());
    let after = persist_after_write(&workspace, store.clone());
    api.after_write = Some(after.clone());
    api.header_identity = false;
    let state = AppState {
        cfg: Arc::new(cfg),
        store,
        pending: Arc::new(PendingLogins::default()),
        http: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .map_err(|e| e.to_string())?,
        mcp: prswt_mcp::Shared {
            workspace,
            after_write: Some(after),
        },
        events: api.events.clone(),
        presence: Arc::new(cloud::Presence::new()),
        desktop_codes: Arc::new(cloud::DesktopCodes::default()),
    };
    let bind = state.cfg.bind;
    let rt = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    let tls = state.cfg.tls.clone();
    rt.block_on(async move {
        let listener = std::net::TcpListener::bind(bind).map_err(|e| e.to_string())?;
        eprintln!(
            "prswt-server listening on {bind} ({}) for {}",
            if tls.is_some() { "https" } else { "http" },
            state.cfg.public_url
        );
        serve(listener, tls, app(state, api), shutdown()).await
    })
}

/// How often the TLS certificate file is checked for renewal.
const CERT_CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3600);

/// Reloads the certificate whenever its file changes, so a renewal (the
/// `cert-renew` container) takes effect without restarting the server. A
/// failed reload keeps serving the previous certificate.
pub async fn watch_certificate(
    config: axum_server::tls_rustls::RustlsConfig,
    cert: PathBuf,
    key: PathBuf,
    every: std::time::Duration,
) {
    let stamp = |p: &PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let mut last = stamp(&cert);
    loop {
        tokio::time::sleep(every).await;
        let now = stamp(&cert);
        if now != last {
            match config.reload_from_pem_file(&cert, &key).await {
                Ok(()) => {
                    eprintln!("prswt-server: reloaded TLS certificate");
                    last = now;
                }
                Err(e) => {
                    eprintln!("prswt-server: certificate reload failed, keeping the old one: {e}");
                }
            }
        }
    }
}

/// Serves `router` on `listener` until `stop` resolves: TLS (rustls) when a
/// PEM certificate/key pair is given, plain HTTP otherwise. With TLS the
/// tunnel's last hop is encrypted and cloudflared can verify the origin.
pub async fn serve(
    listener: std::net::TcpListener,
    tls: Option<(PathBuf, PathBuf)>,
    router: Router,
    stop: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<(), String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    match tls {
        Some((cert, key)) => {
            // Several rustls users share the process; one provider for all.
            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
            let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert, &key)
                .await
                .map_err(|e| format!("TLS certificate/key ({}): {e}", cert.display()))?;
            tokio::spawn(watch_certificate(
                config.clone(),
                cert.clone(),
                key.clone(),
                CERT_CHECK_INTERVAL,
            ));
            let handle = axum_server::Handle::new();
            let h = handle.clone();
            tokio::spawn(async move {
                stop.await;
                h.graceful_shutdown(Some(std::time::Duration::from_secs(10)));
            });
            axum_server::from_tcp_rustls(listener, config)
                .map_err(|e| e.to_string())?
                .handle(handle)
                .serve(router.into_make_service())
                .await
                .map_err(|e| e.to_string())
        }
        None => {
            let listener =
                tokio::net::TcpListener::from_std(listener).map_err(|e| e.to_string())?;
            axum::serve(listener, router)
                .with_graceful_shutdown(stop)
                .await
                .map_err(|e| e.to_string())
        }
    }
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}

/// The full router.
pub fn app(state: AppState, api: prswt_api::ApiState) -> Router {
    let protected_api = prswt_api::router(api).layer(middleware::from_fn_with_state(
        state.clone(),
        require_caller,
    ));
    let cloud_api = Router::new()
        .route("/v1/me", get(cloud::me))
        .route("/v1/me/settings", axum::routing::put(cloud::put_settings))
        .route(
            "/v1/me/token",
            axum::routing::delete(cloud::revoke_own_token),
        )
        .route(
            "/v1/projects/{id}/members",
            get(cloud::members).put(cloud::set_member),
        )
        .route(
            "/v1/projects/{id}/members/{email}",
            axum::routing::delete(cloud::remove_member),
        )
        .route("/v1/projects/{id}/watch", get(cloud::watch))
        .route("/auth/desktop/allow", post(cloud::desktop_allow))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            require_caller,
        ))
        .with_state(state.clone());
    Router::new()
        .route("/", get(home))
        .route("/healthz", get(|| async { "ok" }))
        .route("/login", get(pages::login))
        .route("/auth/login/{provider}", get(login_redirect))
        .route("/auth/callback/{provider}", get(callback))
        .route("/auth/dev-login", post(dev_login))
        .route("/auth/desktop", get(cloud::desktop_start))
        .route("/auth/desktop/token", post(cloud::desktop_token))
        .route("/auth/logout", post(logout))
        .route(
            "/account",
            get(pages::account)
                .layer(middleware::from_fn_with_state(
                    state.clone(),
                    require_caller,
                ))
                .layer(middleware::from_fn_with_state(state.clone(), require_page)),
        )
        .route(
            "/account/tokens",
            post(create_token).layer(middleware::from_fn_with_state(
                state.clone(),
                require_caller,
            )),
        )
        .route(
            "/account/tokens/{id}/revoke",
            post(revoke_token).layer(middleware::from_fn_with_state(
                state.clone(),
                require_caller,
            )),
        )
        .route(
            "/mcp",
            post(mcp_post).get(|| async { StatusCode::METHOD_NOT_ALLOWED }),
        )
        .with_state(state.clone())
        // The API router owns its `/v1/...` paths; merging keeps them intact.
        .merge(protected_api)
        .merge(cloud_api)
        .fallback(|| async { (StatusCode::NOT_FOUND, "not found") })
        .layer(middleware::from_fn(security_headers))
}

// ───────────────────────────── identity ─────────────────────────────

/// Whether `email` may use the server: allowed by configuration, or a
/// member of some project (invited people).
async fn may_sign_in(state: &AppState, email: &str) -> bool {
    if state.cfg.allows(email) {
        return true;
    }
    let (store, e) = (state.store.clone(), email.to_owned());
    tokio::task::spawn_blocking(move || store.is_member_anywhere(&e))
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or(false)
}

/// The projects `email` may use, as the engine's access map. A read-only
/// token gets the owner's projects, each as a viewer.
async fn access_of(state: &AppState, email: &str, read_only: bool) -> Option<Access> {
    let (store, e) = (state.store.clone(), email.to_owned());
    let rows = tokio::task::spawn_blocking(move || store.access_for(&e))
        .await
        .ok()?
        .ok()?;
    Some(
        rows.into_iter()
            .map(|(id, role)| {
                let role = match (read_only, role.as_str()) {
                    (true, _) | (_, "viewer") => Role::Viewer,
                    (_, "owner") => Role::Owner,
                    _ => Role::Editor,
                };
                (prswt_core::ids::ProjectId(id), role)
            })
            .collect(),
    )
}

/// Resolves the caller from `Authorization: Bearer` or the session cookie.
pub(crate) async fn identify(state: &AppState, headers: &HeaderMap) -> Option<Caller> {
    if let Some(token) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|t| t.starts_with(TOKEN_PREFIX) && t.len() < 200)
    {
        let hash = oidc::hash_secret(token);
        let store = state.store.clone();
        let owner = tokio::task::spawn_blocking(move || store.use_token(&hash))
            .await
            .ok()?
            .ok()??;
        if !may_sign_in(state, &owner.email).await {
            return None; // no longer allowed: tokens stop working too
        }
        let access = access_of(state, &owner.email, owner.kind == "read").await?;
        let mut ctx = if owner.kind == "desktop" {
            Ctx::human(&owner.email)
        } else {
            Ctx::agent(&format!("{} ({})", owner.name, owner.email))
        };
        if owner.kind == "read" {
            ctx = ctx.read_only();
        }
        return Some(Caller {
            ctx: ctx.for_account(&owner.email).with_access(access),
            email: owner.email,
            via_cookie: false,
            kind: owner.kind,
            token: Some(owner.id),
        });
    }
    let session = cookie(headers, COOKIE)?;
    let hash = oidc::hash_secret(&session);
    let store = state.store.clone();
    let email = tokio::task::spawn_blocking(move || store.session_email(&hash))
        .await
        .ok()?
        .ok()??;
    if !may_sign_in(state, &email).await {
        return None;
    }
    let access = access_of(state, &email, false).await?;
    Some(Caller {
        ctx: Ctx::human(&email).for_account(&email).with_access(access),
        email,
        via_cookie: true,
        kind: "session".into(),
        token: None,
    })
}

/// Middleware for API and account routes: 401 without identity; cookie-based
/// writes must come from our own origin (CSRF defence on top of SameSite).
async fn require_caller(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let Some(caller) = identify(&state, req.headers()).await else {
        return (
            StatusCode::UNAUTHORIZED,
            "sign in or send Authorization: Bearer <token>",
        )
            .into_response();
    };
    if caller.via_cookie
        && req.method() != Method::GET
        && !same_origin(&state.cfg.public_url, req.headers())
    {
        return (StatusCode::FORBIDDEN, "cross-origin request refused").into_response();
    }
    req.extensions_mut().insert(caller.ctx.clone());
    req.extensions_mut().insert(caller);
    next.run(req).await
}

/// Browser pages: signed-out visitors go to the sign-in page instead of
/// getting a bare 401 (API and MCP keep answering 401).
async fn require_page(State(state): State<AppState>, req: Request, next: Next) -> Response {
    match identify(&state, req.headers()).await {
        Some(c) if c.via_cookie => next.run(req).await,
        _ => Redirect::to("/login").into_response(),
    }
}

/// `/`: the account page when signed in, the sign-in page otherwise. The
/// server has no web UI (ADR-0009); the browser is only for issuing tokens.
async fn home(State(state): State<AppState>, headers: HeaderMap) -> Response {
    match identify(&state, &headers).await {
        Some(c) if c.via_cookie => Redirect::to("/account").into_response(),
        _ => Redirect::to("/login").into_response(),
    }
}

fn same_origin(public: &str, headers: &HeaderMap) -> bool {
    let origin = headers
        .get(header::ORIGIN)
        .or_else(|| headers.get(header::REFERER))
        .and_then(|v| v.to_str().ok());
    origin.is_some_and(|o| o == public || o.starts_with(&format!("{public}/")))
}

fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|kv| kv.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_owned())
}

fn session_cookie(cfg: &Config, value: &str, max_age: i64) -> HeaderValue {
    let secure = if cfg.secure_cookies() { "; Secure" } else { "" };
    HeaderValue::from_str(&format!(
        "{COOKIE}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}"
    ))
    .unwrap_or_else(|_| HeaderValue::from_static("prswt_session=; Path=/; Max-Age=0"))
}

/// Where to go after signing in: a local path from the sign-in request,
/// else home. Only same-site paths are followed (no open redirect).
fn after_login(next: Option<&str>) -> String {
    match next {
        Some(n) if n.starts_with('/') && !n.starts_with("//") && !n.contains('\\') => n.to_owned(),
        _ => "/".to_owned(),
    }
}

/// Starts a session for `email` and redirects to `next` (or home).
async fn start_session(
    state: &AppState,
    email: String,
    provider: &str,
    next: Option<&str>,
) -> Response {
    let Some(secret) = oidc::random_token(32) else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "no randomness available").into_response();
    };
    let hash = oidc::hash_secret(&secret);
    let (store, hours, provider) = (
        state.store.clone(),
        state.cfg.session_hours,
        provider.to_owned(),
    );
    let saved =
        tokio::task::spawn_blocking(move || store.create_session(&hash, &email, &provider, hours))
            .await;
    if !matches!(saved, Ok(Ok(()))) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not create the session",
        )
            .into_response();
    }
    let mut resp = Redirect::to(&after_login(next)).into_response();
    resp.headers_mut().insert(
        header::SET_COOKIE,
        session_cookie(&state.cfg, &secret, i64::from(hours) * 3600),
    );
    resp
}

/// `?next=` on sign-in routes.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct NextQuery {
    #[serde(default)]
    next: Option<String>,
}

// ───────────────────────────── OIDC routes ─────────────────────────────

async fn login_redirect(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    Query(nq): Query<NextQuery>,
) -> Response {
    let Some(p) = state.cfg.providers.iter().find(|p| p.name == provider) else {
        return (StatusCode::NOT_FOUND, "unknown sign-in provider").into_response();
    };
    let (Some(st), Some(verifier), Some(nonce)) = (
        oidc::random_token(24),
        oidc::random_token(32),
        oidc::random_token(16),
    ) else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "no randomness available").into_response();
    };
    let redirect_uri = format!("{}/auth/callback/{}", state.cfg.public_url, p.name);
    let url = oidc::authorize_url(
        p,
        &redirect_uri,
        &st,
        &nonce,
        &oidc::pkce_challenge(&verifier),
    );
    if !state.pending.insert(
        st,
        Pending {
            provider: p.name,
            verifier,
            nonce,
            started: unix_now(),
            next: nq.next.map(|n| n.chars().take(512).collect()),
        },
        unix_now(),
    ) {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "too many sign-ins in progress; try again",
        )
            .into_response();
    }
    Redirect::to(&url).into_response()
}

/// Query of the provider's redirect back to us.
#[derive(Debug, Deserialize)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

async fn callback(
    State(state): State<AppState>,
    Path(provider): Path<String>,
    Query(q): Query<CallbackQuery>,
) -> Response {
    if let Some(e) = q.error {
        return pages::message("Sign-in cancelled", &format!("The provider reported: {e}"));
    }
    let (Some(code), Some(st)) = (q.code, q.state) else {
        return pages::message("Sign-in failed", "Missing code or state.");
    };
    let Some(pending) = state
        .pending
        .take(&st, unix_now())
        .filter(|p| p.provider == provider)
    else {
        return pages::message("Sign-in expired", "Please start the sign-in again.");
    };
    let Some(p) = state.cfg.providers.iter().find(|p| p.name == provider) else {
        return pages::message("Sign-in failed", "Unknown provider.");
    };
    let redirect_uri = format!("{}/auth/callback/{}", state.cfg.public_url, p.name);
    let claims = match oidc::exchange(&state.http, p, &code, &redirect_uri, &pending.verifier).await
    {
        Ok(c) => c,
        Err(e) => return pages::message("Sign-in failed", &e),
    };
    let email = match oidc::validate_claims(&claims, p, &pending.nonce, unix_now()) {
        Ok(e) => e,
        Err(e) => return pages::message("Sign-in failed", &e),
    };
    if !may_sign_in(&state, &email).await {
        return pages::message(
            "Not allowed",
            &format!(
                "{email} is not on this server's access list and has not been invited to a project."
            ),
        );
    }
    start_session(&state, email, p.name, pending.next.as_deref()).await
}

async fn dev_login(State(state): State<AppState>, Query(nq): Query<NextQuery>) -> Response {
    match state.cfg.dev_login.clone() {
        Some(email) if state.cfg.allows(&email.to_ascii_lowercase()) => {
            start_session(
                &state,
                email.to_ascii_lowercase(),
                "dev",
                nq.next.as_deref(),
            )
            .await
        }
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(s) = cookie(&headers, COOKIE) {
        let (store, hash) = (state.store.clone(), oidc::hash_secret(&s));
        let _ = tokio::task::spawn_blocking(move || store.delete_session(&hash)).await;
    }
    let mut resp = Redirect::to("/login").into_response();
    resp.headers_mut()
        .insert(header::SET_COOKIE, session_cookie(&state.cfg, "", 0));
    resp
}

// ───────────────────────────── tokens ─────────────────────────────

/// Form posted from the account page.
#[derive(Debug, Deserialize)]
struct NewToken {
    name: String,
    /// `agent` (sandbox work, default) or `read` (read-only).
    #[serde(default)]
    kind: Option<String>,
}

async fn create_token(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Form(f): Form<NewToken>,
) -> Response {
    if !caller.via_cookie {
        return (
            StatusCode::FORBIDDEN,
            "tokens are created from a signed-in browser",
        )
            .into_response();
    }
    let name: String = f
        .name
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || "-_. ".contains(*c))
        .take(40)
        .collect();
    let name = if name.is_empty() {
        "token".to_owned()
    } else {
        name
    };
    let Some(secret) = oidc::random_token(32).map(|s| format!("{TOKEN_PREFIX}{s}")) else {
        return (StatusCode::INTERNAL_SERVER_ERROR, "no randomness available").into_response();
    };
    let kind = if f.kind.as_deref() == Some("read") {
        "read"
    } else {
        "agent"
    };
    let (store, hash, email, n) = (
        state.store.clone(),
        oidc::hash_secret(&secret),
        caller.email.clone(),
        name.clone(),
    );
    match tokio::task::spawn_blocking(move || {
        store.create_token(uuid::Uuid::now_v7(), &hash, &email, &n, kind)
    })
    .await
    {
        Ok(Ok(())) => pages::new_token(&state.cfg.public_url, &name, &secret),
        Ok(Err(e)) => pages::message("Token not created", &e.message),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "token task failed").into_response(),
    }
}

async fn revoke_token(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<uuid::Uuid>,
) -> Response {
    let store = state.store.clone();
    let _ = tokio::task::spawn_blocking(move || store.revoke_token(id, &caller.email)).await;
    Redirect::to("/account").into_response()
}

// ───────────────────────────── remote MCP ─────────────────────────────

/// MCP streamable HTTP (MCP-13): one JSON-RPC message per POST, JSON reply;
/// notifications get 202. Bearer tokens only; callers act as agents.
async fn mcp_post(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    // MCP callers are agents: browser sessions and the desktop app's
    // personal token do not count.
    let Some(caller) = identify(&state, &headers)
        .await
        .filter(|c| !c.via_cookie && c.kind != "desktop")
    else {
        let mut r = (
            StatusCode::UNAUTHORIZED,
            "send Authorization: Bearer <token> (create one at /account)",
        )
            .into_response();
        r.headers_mut()
            .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        return r;
    };
    let Ok(text) = String::from_utf8(body.to_vec()) else {
        return (StatusCode::BAD_REQUEST, "body must be UTF-8 JSON").into_response();
    };
    let backend = state.mcp.clone();
    let reply = tokio::task::spawn_blocking(move || {
        prswt_mcp::Server::with_identity(backend, caller.ctx).handle_line(&text)
    })
    .await;
    match reply {
        Ok(Some(json)) => ([(header::CONTENT_TYPE, "application/json")], json).into_response(),
        Ok(None) => StatusCode::ACCEPTED.into_response(),
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "MCP task failed").into_response(),
    }
}

// ───────────────────────────── hardening ─────────────────────────────

async fn security_headers(req: Request, next: Next) -> Response {
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    h.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    h.insert("referrer-policy", HeaderValue::from_static("same-origin"));
    h.insert("content-security-policy", HeaderValue::from_static(CSP));
    resp
}

#[cfg(test)]
mod tests;
