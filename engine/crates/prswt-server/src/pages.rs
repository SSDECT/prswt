//! Server-rendered HTML for sign-in and token management. No JavaScript;
//! every dynamic value is HTML-escaped.

use std::fmt::Write as _;

use axum::Extension;
use axum::extract::{Query, State};
use axum::response::{Html, IntoResponse, Response};

use crate::{AppState, Caller};

/// Shared page chrome (inline CSS only; CSP allows inline styles).
fn page(title: &str, body: &str) -> Response {
    Html(format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
<title>{t} · PRSWT</title><style>\
:root{{color-scheme:light dark;--bg:#f6f7f9;--fg:#1d2330;--card:#fff;--muted:#5b6475;--accent:#2a64c8;--line:#d9dde5}}\
@media (prefers-color-scheme:dark){{:root{{--bg:#12151b;--fg:#e4e8f0;--card:#1b2029;--muted:#9aa3b2;--accent:#6ea0ff;--line:#2c3340}}}}\
body{{margin:0;font:15px/1.5 system-ui,sans-serif;background:var(--bg);color:var(--fg)}}\
main{{max-width:640px;margin:8vh auto;padding:0 16px}}.card{{background:var(--card);border:1px solid var(--line);border-radius:12px;padding:28px}}\
h1{{font-size:22px;margin:0 0 6px}}p{{color:var(--muted)}}form{{margin:10px 0}}\
button,.btn{{display:inline-block;padding:10px 16px;border-radius:8px;border:1px solid var(--line);background:var(--accent);color:#fff;font:inherit;cursor:pointer;text-decoration:none}}\
button.secondary{{background:transparent;color:var(--fg)}}input{{padding:9px;border-radius:8px;border:1px solid var(--line);background:transparent;color:inherit;font:inherit}}\
table{{width:100%;border-collapse:collapse;margin-top:12px}}td,th{{text-align:left;padding:6px 4px;border-bottom:1px solid var(--line);font-size:14px}}\
code{{word-break:break-all;background:rgba(127,127,127,.15);padding:2px 5px;border-radius:5px}}</style></head>\
<body><main><div class=\"card\">{body}</div></main></body></html>",
        t = esc(title),
    ))
    .into_response()
}

/// A simple titled message.
pub fn message(title: &str, text: &str) -> Response {
    page(
        title,
        &format!(
            "<h1>{}</h1><p>{}</p><p><a href=\"/login\">Back to sign-in</a></p>",
            esc(title),
            esc(text)
        ),
    )
}

/// Sign-in page with one button per configured provider. `?next=` is
/// passed on, so the desktop sign-in continues after signing in.
pub async fn login(State(state): State<AppState>, Query(nq): Query<crate::NextQuery>) -> Response {
    let next = nq
        .next
        .as_deref()
        .map(|n| format!("?next={}", crate::cloud::urlencode(n)))
        .unwrap_or_default();
    let mut buttons: String = state
        .cfg
        .providers
        .iter()
        .map(|p| {
            format!(
                "<p><a class=\"btn\" href=\"/auth/login/{}{}\">{}</a></p>",
                esc(p.name),
                esc(&next),
                esc(p.label)
            )
        })
        .collect();
    if state.cfg.dev_login.is_some() {
        let _ = write!(
            buttons,
            "<form method=\"post\" action=\"/auth/dev-login{}\"><button class=\"secondary\">Development sign-in</button></form>",
            esc(&next)
        );
    }
    page(
        "Sign in",
        &format!(
            "<h1>PRSWT</h1><p>Project controls &amp; risk engine. Sign in with an approved account.</p>{buttons}"
        ),
    )
}

/// Account page: who you are, your access tokens, logout.
pub async fn account(
    State(state): State<AppState>,
    Extension(caller): Extension<Caller>,
) -> Response {
    let store = state.store.clone();
    let email = caller.email.clone();
    let tokens = tokio::task::spawn_blocking(move || store.list_tokens(&email))
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    let rows: String = tokens
        .iter()
        .map(|t| {
            let status = match (&t.revoked_at, &t.last_used_at) {
                (Some(_), _) => "revoked".to_owned(),
                (None, Some(u)) => format!("last used {}", esc(&u[..u.len().min(16)])),
                (None, None) => "never used".to_owned(),
            };
            let action = if t.revoked_at.is_none() {
                format!("<form method=\"post\" action=\"/account/tokens/{}/revoke\"><button class=\"secondary\">Revoke</button></form>", t.id)
            } else {
                String::new()
            };
            let kind = match t.kind.as_str() {
                "desktop" => "desktop app",
                "read" => "read only",
                _ => "agent",
            };
            format!("<tr><td>{}</td><td>{kind}</td><td>{}</td><td>{status}</td><td>{action}</td></tr>", esc(&t.name), esc(&t.created_at[..t.created_at.len().min(16)]))
        })
        .collect();
    page(
        "Account",
        &format!(
            "<h1>Account</h1><p>Signed in as <b>{email}</b>. <a href=\"/\">Open PRSWT</a></p>\
<h2>Access tokens</h2><p>Tokens let Claude (MCP) and scripts use PRSWT as <i>agents</i>: they work in sandboxes and need you to commit to the live plan.</p>\
<form method=\"post\" action=\"/account/tokens\"><input name=\"name\" placeholder=\"e.g. claude-laptop\" maxlength=\"40\" required> \
<select name=\"kind\"><option value=\"agent\">Works in sandboxes</option><option value=\"read\">Read only</option></select> <button>Create token</button></form>\
<table><tr><th>Name</th><th>Kind</th><th>Created (UTC)</th><th>Status</th><th></th></tr>{rows}</table>\
<form method=\"post\" action=\"/auth/logout\"><button class=\"secondary\">Sign out</button></form>",
            email = esc(&caller.email),
        ),
    )
}

/// Asks the signed-in person to let the desktop app use their account.
pub fn desktop_consent(email: &str, port: u16, state: &str, challenge: &str) -> Response {
    page(
        "Sign in PRSWT desktop",
        &format!(
            "<h1>Sign in to PRSWT desktop</h1><p>The PRSWT app on this computer asks to use your account <b>{e}</b> \
to open and sync your cloud projects. You can sign it out at any time on your <a href=\"/account\">account page</a>.</p>\
<p><b>Click Allow only if you just chose “Sign in” in PRSWT on this computer.</b> \
If you did not, a program may be trying to use your account: click Cancel. \
(The app waits on local port {port}.)</p>\
<form method=\"post\" action=\"/auth/desktop/allow\">\
<input type=\"hidden\" name=\"port\" value=\"{port}\"><input type=\"hidden\" name=\"state\" value=\"{s}\">\
<input type=\"hidden\" name=\"challenge\" value=\"{c}\"><button>Allow</button> <a href=\"/account\">Cancel</a></form>",
            e = esc(email),
            s = esc(state),
            c = esc(challenge),
        ),
    )
}

/// Shows a freshly created token exactly once, with the Claude Code command.
pub fn new_token(public_url: &str, name: &str, secret: &str) -> Response {
    page(
        "New token",
        &format!(
            "<h1>Token “{n}” created</h1><p>Copy it now; it is not shown again.</p><p><code>{s}</code></p>\
<p>Connect Claude Code:</p><p><code>claude mcp add --transport http prswt {u}/mcp --header \"Authorization: Bearer {s}\"</code></p>\
<p><a href=\"/account\">Back to account</a></p>",
            n = esc(name),
            s = esc(secret),
            u = esc(public_url),
        ),
    )
}

/// Minimal HTML escaping for text and attribute values.
pub fn esc(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '&' => "&amp;".to_owned(),
            '<' => "&lt;".to_owned(),
            '>' => "&gt;".to_owned(),
            '"' => "&quot;".to_owned(),
            '\'' => "&#39;".to_owned(),
            c => c.to_string(),
        })
        .collect()
}
