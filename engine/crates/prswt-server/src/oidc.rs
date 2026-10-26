//! AUTH-01: OpenID Connect sign-in (Google, Microsoft) with the
//! Authorization Code flow, PKCE (S256), `state` and `nonce`.
//!
//! The ID token is received directly from the provider's token endpoint over
//! TLS, so per OIDC Core §3.1.3.7 (6) TLS server validation stands in for the
//! signature check; issuer, audience, expiry, nonce and email are still
//! validated here ([`validate_claims`]).

use std::collections::HashMap;
use std::sync::Mutex;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// How long a login attempt may take between redirect and callback.
const PENDING_TTL_SECS: u64 = 600;
/// Most simultaneous pending logins kept (memory guard).
const MAX_PENDING: usize = 10_000;

/// One configured identity provider. `Debug` hides the client secret so a
/// stray `{:?}` of the config never writes it to a log.
#[derive(Clone)]
pub struct Provider {
    /// `google` or `microsoft` (used in URLs).
    pub name: &'static str,
    /// Button label.
    pub label: &'static str,
    /// Authorization endpoint.
    pub auth_url: String,
    /// Token endpoint.
    pub token_url: String,
    /// OAuth client id.
    pub client_id: String,
    /// OAuth client secret.
    pub client_secret: String,
    /// Microsoft: the directories (tenant ids, `tid`) whose accounts may
    /// sign in. Empty for Google.
    pub tenants: Vec<String>,
    /// Microsoft: sign-in is open to several directories (`common` /
    /// `organizations`). Then an email is trusted only when its directory
    /// has verified it (`xms_edov`), since anyone can put any address on
    /// an account in their own directory ("nOAuth").
    pub multi_tenant: bool,
}

impl std::fmt::Debug for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Provider")
            .field("name", &self.name)
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("tenants", &self.tenants)
            .finish_non_exhaustive()
    }
}

impl Provider {
    /// Google with the given client credentials.
    pub fn google(client_id: String, client_secret: String) -> Self {
        Self {
            name: "google",
            label: "Sign in with Google",
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth".into(),
            token_url: "https://oauth2.googleapis.com/token".into(),
            client_id,
            client_secret,
            tenants: Vec::new(),
            multi_tenant: false,
        }
    }

    /// Microsoft identity platform. `tenant` is a tenant id (one
    /// organization), or `common` / `organizations` (several), in which case
    /// `allowed` lists the tenant ids that may sign in; an empty list is
    /// refused (any directory could sign in with any email).
    pub fn microsoft(
        client_id: String,
        client_secret: String,
        tenant: &str,
        allowed: &[String],
    ) -> Result<Self, String> {
        let multi_tenant = matches!(tenant, "common" | "organizations" | "consumers");
        let tenants: Vec<String> = if multi_tenant {
            allowed
                .iter()
                .map(|t| t.trim().to_ascii_lowercase())
                .filter(|t| !t.is_empty())
                .collect()
        } else {
            vec![tenant.to_ascii_lowercase()]
        };
        if tenants.is_empty() {
            return Err(format!(
                "MICROSOFT_TENANT={tenant} needs MICROSOFT_ALLOWED_TENANTS (the tenant ids that may sign in)"
            ));
        }
        Ok(Self {
            name: "microsoft",
            label: "Sign in with Microsoft",
            auth_url: format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/authorize"),
            token_url: format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token"),
            client_id,
            client_secret,
            tenants,
            multi_tenant,
        })
    }

    /// Whether `iss` is a valid issuer for this provider.
    pub fn issuer_ok(&self, iss: &str) -> bool {
        match self.name {
            "google" => iss == "https://accounts.google.com" || iss == "accounts.google.com",
            "microsoft" => {
                iss.starts_with("https://login.microsoftonline.com/") && iss.ends_with("/v2.0")
            }
            _ => false,
        }
    }
}

/// A login in progress (between redirect and callback).
#[derive(Debug, Clone)]
pub struct Pending {
    /// Provider name.
    pub provider: &'static str,
    /// PKCE code verifier.
    pub verifier: String,
    /// Expected `nonce` claim.
    pub nonce: String,
    /// Unix seconds when the login started.
    pub started: u64,
    /// Local path to open after signing in (desktop sign-in), if any.
    pub next: Option<String>,
}

/// Pending logins keyed by `state`.
#[derive(Debug, Default)]
pub struct PendingLogins(Mutex<HashMap<String, Pending>>);

impl PendingLogins {
    /// Remembers a login; drops expired ones and, when full, the oldest
    /// (a flood of started logins must not lock everyone out).
    pub fn insert(&self, state: String, p: Pending, now: u64) -> bool {
        let Ok(mut m) = self.0.lock() else {
            return false;
        };
        m.retain(|_, v| now.saturating_sub(v.started) < PENDING_TTL_SECS);
        if m.len() >= MAX_PENDING
            && let Some(oldest) = m
                .iter()
                .min_by_key(|(_, v)| v.started)
                .map(|(k, _)| k.clone())
        {
            m.remove(&oldest);
        }
        m.insert(state, p);
        true
    }

    /// Takes (and forgets) the login for `state` if it has not expired.
    pub fn take(&self, state: &str, now: u64) -> Option<Pending> {
        let p = self.0.lock().ok()?.remove(state)?;
        (now.saturating_sub(p.started) < PENDING_TTL_SECS).then_some(p)
    }
}

/// `n` random bytes from the operating system, URL-safe base64.
pub fn random_token(n: usize) -> Option<String> {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf).ok()?;
    Some(URL_SAFE_NO_PAD.encode(buf))
}

/// PKCE S256 challenge for a verifier.
pub fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Hex SHA-256 of a secret (what the database stores).
pub fn hash_secret(secret: &str) -> String {
    Sha256::digest(secret.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Authorization redirect URL.
pub fn authorize_url(
    p: &Provider,
    redirect_uri: &str,
    state: &str,
    nonce: &str,
    challenge: &str,
) -> String {
    let q = [
        ("response_type", "code"),
        ("client_id", p.client_id.as_str()),
        ("redirect_uri", redirect_uri),
        ("scope", "openid email profile"),
        ("state", state),
        ("nonce", nonce),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("prompt", "select_account"),
    ];
    let query: Vec<String> = q
        .iter()
        .map(|(k, v)| format!("{k}={}", encode(v)))
        .collect();
    format!("{}?{}", p.auth_url, query.join("&"))
}

/// Exchanges the authorization code for an ID token (claims returned decoded).
pub async fn exchange(
    http: &reqwest::Client,
    p: &Provider,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> Result<Value, String> {
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", p.client_id.as_str()),
        ("client_secret", p.client_secret.as_str()),
        ("code_verifier", verifier),
    ];
    let resp = http
        .post(&p.token_url)
        .form(&form)
        .send()
        .await
        .map_err(|e| format!("token request failed: {e}"))?;
    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .map_err(|e| format!("token response unreadable: {e}"))?;
    if !status.is_success() {
        let reason = body
            .get("error_description")
            .or_else(|| body.get("error"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        return Err(format!("token endpoint refused ({status}): {reason}"));
    }
    let id_token = body
        .get("id_token")
        .and_then(Value::as_str)
        .ok_or("no id_token in token response")?;
    decode_jwt_payload(id_token)
}

/// Payload of a compact JWT, without signature verification (see module docs).
pub fn decode_jwt_payload(jwt: &str) -> Result<Value, String> {
    let payload = jwt.split('.').nth(1).ok_or("malformed ID token")?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .map_err(|_| "ID token is not base64url")?;
    serde_json::from_slice(&bytes).map_err(|_| "ID token payload is not JSON".into())
}

/// Validates ID-token claims; returns the lower-cased, verified email.
pub fn validate_claims(c: &Value, p: &Provider, nonce: &str, now: u64) -> Result<String, String> {
    let iss = c.get("iss").and_then(Value::as_str).unwrap_or("");
    if !p.issuer_ok(iss) {
        return Err(format!("unexpected issuer `{iss}`"));
    }
    let aud_ok = match c.get("aud") {
        Some(Value::String(a)) => *a == p.client_id,
        Some(Value::Array(a)) => a.iter().any(|x| x.as_str() == Some(p.client_id.as_str())),
        _ => false,
    };
    if !aud_ok {
        return Err("token was issued for another application".into());
    }
    let exp = c.get("exp").and_then(Value::as_u64).unwrap_or(0);
    if exp <= now {
        return Err("token expired".into());
    }
    if c.get("nonce").and_then(Value::as_str) != Some(nonce) {
        return Err("nonce mismatch (possible replay)".into());
    }
    let email = match p.name {
        "google" => {
            if c.get("email_verified").and_then(Value::as_bool) != Some(true) {
                return Err("Google account email is not verified".into());
            }
            c.get("email").and_then(Value::as_str)
        }
        _ => {
            let tid = c
                .get("tid")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase();
            if !p.tenants.contains(&tid) {
                return Err("accounts of this organization may not sign in here".into());
            }
            if p.multi_tenant {
                // Several directories: only an email the directory verified.
                if c.get("xms_edov").and_then(Value::as_bool) != Some(true) {
                    return Err("the account's email is not verified by its organization".into());
                }
                c.get("email").and_then(Value::as_str)
            } else {
                // One directory, run by its admins: its sign-in name will do.
                c.get("email")
                    .and_then(Value::as_str)
                    .or_else(|| c.get("preferred_username").and_then(Value::as_str))
            }
        }
    };
    let email = email
        .filter(|e| e.contains('@') && e.len() <= 254)
        .ok_or("no email address in the token")?;
    Ok(email.to_ascii_lowercase())
}

/// Percent-encodes a query value (RFC 3986 unreserved characters kept).
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}
