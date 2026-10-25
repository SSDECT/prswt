//! Security unit tests, plus an end-to-end router test against the test
//! database (needs `PRSWT_TEST_DATABASE_URL`, skipped without it).

use std::collections::HashSet;

use axum::body::{Body, to_bytes};
use axum::http::Request;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::*;
use crate::oidc::{decode_jwt_payload, pkce_challenge, validate_claims};

mod oidc_rules {
    use super::*;

    fn google() -> Provider {
        Provider::google("client-1".into(), "secret".into())
    }

    fn claims() -> Value {
        json!({ "iss": "https://accounts.google.com", "aud": "client-1", "exp": 2_000, "nonce": "n1", "email": "Ruslan@Juz.ai", "email_verified": true })
    }

    #[test]
    fn auth_01_pkce_s256_reference() {
        // Reference computed independently with Python: base64url(sha256(verifier)), no padding.
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mJ92K9xF8x7XbpTwnTxvFYgOs1C3rU"),
            "yYq_c9ru7XG7RKsaJLBMOZt0IxmNMY8Ee50eFX1e14Y"
        );
    }

    #[test]
    fn auth_01_valid_claims_give_lowercase_email() {
        assert_eq!(
            validate_claims(&claims(), &google(), "n1", 1_000),
            Ok("ruslan@juz.ai".into())
        );
    }

    #[test]
    fn auth_01_rejections() {
        let p = google();
        let bad = |k: &str, v: Value| {
            let mut c = claims();
            c[k] = v;
            validate_claims(&c, &p, "n1", 1_000).unwrap_err()
        };
        assert!(bad("iss", json!("https://evil.example")).contains("issuer"));
        assert!(bad("aud", json!("other-app")).contains("another application"));
        assert!(bad("exp", json!(999)).contains("expired"));
        assert!(bad("nonce", json!("replayed")).contains("nonce"));
        assert!(bad("email_verified", json!(false)).contains("not verified"));
        assert!(bad("email", json!("no-at-sign")).contains("no email"));
        // One directory: its sign-in name is trusted; other directories are not.
        let ms = Provider::microsoft("client-1".into(), "s".into(), "T-1", &[]).unwrap();
        let c = json!({ "iss": "https://login.microsoftonline.com/t-1/v2.0", "aud": ["client-1"], "exp": 2_000, "nonce": "n1", "tid": "t-1", "preferred_username": "a@b.c" });
        assert_eq!(validate_claims(&c, &ms, "n1", 1_000), Ok("a@b.c".into()));
        let mut other = c.clone();
        other["tid"] = json!("t-2");
        assert!(
            validate_claims(&other, &ms, "n1", 1_000)
                .unwrap_err()
                .contains("organization")
        );
    }

    /// Review M5 ("nOAuth"): with several directories an email counts only
    /// when its directory verified it, and only listed directories sign in.
    #[test]
    fn auth_01_multi_tenant_needs_verified_email() {
        assert!(
            Provider::microsoft("c".into(), "s".into(), "common", &[]).is_err(),
            "common without an allowlist is refused"
        );
        let ms = Provider::microsoft(
            "client-1".into(),
            "s".into(),
            "organizations",
            &["T-1".into(), "t-2".into()],
        )
        .unwrap();
        let c = json!({ "iss": "https://login.microsoftonline.com/t-2/v2.0", "aud": "client-1", "exp": 2_000, "nonce": "n1", "tid": "t-2", "email": "Boss@Juz.ai", "preferred_username": "boss@juz.ai" });
        assert!(
            validate_claims(&c, &ms, "n1", 1_000)
                .unwrap_err()
                .contains("not verified"),
            "an unverified email from another directory could be anyone's"
        );
        let mut ok = c.clone();
        ok["xms_edov"] = json!(true);
        assert_eq!(
            validate_claims(&ok, &ms, "n1", 1_000),
            Ok("boss@juz.ai".into())
        );
        let mut no_email = ok.clone();
        no_email.as_object_mut().unwrap().remove("email");
        assert!(
            validate_claims(&no_email, &ms, "n1", 1_000).is_err(),
            "no fallback to the sign-in name"
        );
        let mut stranger = ok;
        stranger["tid"] = json!("t-3");
        assert!(validate_claims(&stranger, &ms, "n1", 1_000).is_err());
    }

    #[test]
    fn auth_01_jwt_payload_and_pending_logins() {
        let payload = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            br#"{"a":1}"#,
        );
        assert_eq!(
            decode_jwt_payload(&format!("h.{payload}.s")).unwrap()["a"],
            1
        );
        assert!(decode_jwt_payload("nonsense").is_err());
        let pl = PendingLogins::default();
        let p = Pending {
            provider: "google",
            verifier: "v".into(),
            nonce: "n".into(),
            started: 100,
            next: None,
        };
        assert!(pl.insert("s1".into(), p.clone(), 100));
        assert!(pl.take("s1", 100 + 601).is_none(), "expired");
        assert!(pl.insert("s2".into(), p, 100));
        assert!(pl.take("s2", 150).is_some());
        assert!(pl.take("s2", 150).is_none(), "single use");
    }

    #[test]
    fn srv_03_csp_allows_no_scripts() {
        assert!(!CSP.contains("script-src"), "{CSP}");
        assert!(CSP.starts_with("default-src 'none'"));
        assert!(HeaderValue::from_str(CSP).is_ok());
    }

    #[test]
    fn auth_02_cookie_origin_and_escaping() {
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            HeaderValue::from_static("a=1; prswt_session=abc; b=2"),
        );
        assert_eq!(cookie(&h, COOKIE).as_deref(), Some("abc"));
        h.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://prswt.juz.ai"),
        );
        assert!(same_origin("https://prswt.juz.ai", &h));
        h.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://prswt.juz.ai.evil.com"),
        );
        assert!(!same_origin("https://prswt.juz.ai", &h));
        assert_eq!(
            pages::esc("<a href=\"x\">&'"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn srv_01_end_to_end_router() {
    let Some((app, _state, origin, _email)) = test_app().await else {
        return;
    };
    let origin = origin.as_str();

    let (code, _, _) = send(
        &app,
        "POST",
        "/v1/commands/schedule.run",
        None,
        None,
        None,
        "",
    )
    .await;
    assert_eq!(code, 401, "API needs identity");
    let (code, headers, _) = send(&app, "GET", "/", None, None, None, "").await;
    assert_eq!(
        (code, headers.get("location").and_then(|v| v.to_str().ok())),
        (303, Some("/login")),
        "signed out: home goes to sign-in"
    );
    let (code, _, _) = send(&app, "GET", "/index.html", None, None, None, "").await;
    assert_eq!(code, 404, "no web UI any more");
    let (code, h, _) = send(&app, "GET", "/account", None, None, None, "").await;
    assert_eq!(
        (code, h.get("location").and_then(|v| v.to_str().ok())),
        (303, Some("/login")),
        "signed-out account page goes to sign-in"
    );
    assert_eq!(
        headers.get("x-frame-options").and_then(|v| v.to_str().ok()),
        Some("DENY")
    );

    let (code, headers, _) = send(
        &app,
        "POST",
        "/auth/dev-login",
        None,
        None,
        Some(origin),
        "",
    )
    .await;
    assert_eq!(code, 303);
    let set = headers
        .get("set-cookie")
        .and_then(|v| v.to_str().ok())
        .unwrap()
        .to_owned();
    assert!(set.contains("HttpOnly") && set.contains("SameSite=Lax"));
    let session = set.split(';').next().unwrap().to_owned();

    let (code, _, body) = send(
        &app,
        "POST",
        "/v1/commands/sample.load",
        Some(&session),
        None,
        None,
        r#"{"name":"bridge"}"#,
    )
    .await;
    assert_eq!(code, 403, "cookie write without Origin is refused: {body}");
    let (code, _, body) = send(
        &app,
        "POST",
        "/v1/commands/sample.load",
        Some(&session),
        None,
        Some(origin),
        r#"{"name":"bridge"}"#,
    )
    .await;
    assert_eq!(code, 200, "{body}");
    let (code, _, body) = send(
        &app,
        "POST",
        "/v1/commands/activity.update",
        Some(&session),
        None,
        Some(origin),
        r#"{"activity":"C30","duration":"16d"}"#,
    )
    .await;
    assert_eq!(code, 200, "humans may edit live: {body}");

    let (code, _, page) = send_form(&app, "/account/tokens", &session, origin, "name=e2e").await;
    assert_eq!(code, 200);
    let token = page
        .split("<code>")
        .nth(1)
        .unwrap()
        .split("</code>")
        .next()
        .unwrap()
        .to_owned();
    assert!(token.starts_with(TOKEN_PREFIX));

    let (code, _, body) = send(
        &app,
        "POST",
        "/v1/commands/activity.update",
        None,
        Some(&token),
        None,
        r#"{"activity":"C30","duration":"9d"}"#,
    )
    .await;
    assert_eq!(code, 403, "token callers are agents: {body}");
    let init = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "clientInfo": { "name": "x" } } }).to_string();
    let (code, _, body) = send(&app, "POST", "/mcp", None, Some(&token), None, &init).await;
    assert_eq!(code, 200, "{body}");
    let (code, _, _) = send(&app, "POST", "/mcp", Some(&session), None, None, &init).await;
    assert_eq!(code, 401, "MCP needs a token, not a cookie");
    let note = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string();
    let (code, _, _) = send(&app, "POST", "/mcp", None, Some(&token), None, &note).await;
    assert_eq!(code, 202);

    let (_, _, account) = send(&app, "GET", "/account", Some(&session), None, None, "").await;
    let id = account
        .split("/account/tokens/")
        .nth(1)
        .unwrap()
        .split('/')
        .next()
        .unwrap()
        .to_owned();
    let (code, _, _) = send(
        &app,
        "POST",
        &format!("/account/tokens/{id}/revoke"),
        Some(&session),
        None,
        Some(origin),
        "",
    )
    .await;
    assert_eq!(code, 303);
    let (code, _, _) = send(&app, "POST", "/mcp", None, Some(&token), None, &init).await;
    assert_eq!(code, 401, "revoked");

    let (code, _, _) = send(
        &app,
        "POST",
        "/auth/logout",
        Some(&session),
        None,
        Some(origin),
        "",
    )
    .await;
    assert_eq!(code, 303);
    let (code, _, _) = send(
        &app,
        "POST",
        "/v1/commands/schedule.run",
        Some(&session),
        None,
        Some(origin),
        "",
    )
    .await;
    assert_eq!(code, 401, "logged out");
}

/// SYNC-01/04: two people, one project. Only members see it, roles hold per
/// command, owners share it, and the desktop app signs in through the
/// browser (loopback + PKCE) to get a personal token.
#[tokio::test(flavor = "multi_thread")]
async fn sync_01_sharing_roles_and_desktop_sign_in() {
    let Some((app, state, origin, alice)) = test_app().await else {
        return;
    };
    let bob = format!("bob-{}@other.test", uuid::Uuid::now_v7());
    let a = session_for(&state, &alice).await;
    let b = session_for(&state, &bob).await;

    // Bob is not invited: he cannot use the server at all yet.
    let (code, _, _) = send(&app, "GET", "/v1/me", Some(&b), None, None, "").await;
    assert_eq!(code, 401, "not allowed and not a member");

    let (code, _, body) = send(
        &app,
        "POST",
        "/v1/commands/sample.load",
        Some(&a),
        None,
        Some(&origin),
        r#"{"name":"bridge"}"#,
    )
    .await;
    assert_eq!(code, 200, "{body}");
    let pid = serde_json::from_str::<Value>(&body).unwrap()["project"]
        .as_str()
        .unwrap()
        .to_owned();
    let (_, _, members) = send(
        &app,
        "GET",
        &format!("/v1/projects/{pid}/members"),
        Some(&a),
        None,
        None,
        "",
    )
    .await;
    assert!(
        members.contains(&alice) && members.contains("owner"),
        "{members}"
    );

    // Alice invites Bob as a viewer: he signs in, reads, cannot write.
    let invite = json!({ "email": bob.to_uppercase(), "role": "viewer" }).to_string();
    let (code, _, body) = send(
        &app,
        "PUT",
        &format!("/v1/projects/{pid}/members"),
        Some(&a),
        None,
        Some(&origin),
        &invite,
    )
    .await;
    assert_eq!(code, 200, "{body}");
    let (code, _, body) = send(
        &app,
        "POST",
        "/v1/commands/project.list",
        Some(&b),
        None,
        Some(&origin),
        r#"{"brief":true}"#,
    )
    .await;
    assert_eq!(code, 200, "{body}");
    assert!(body.contains(&pid), "{body}");
    let edit = r#"{"activity":"C30","duration":"17d"}"#;
    let (code, _, _) = send(
        &app,
        "POST",
        "/v1/commands/activity.update",
        Some(&b),
        None,
        Some(&origin),
        edit,
    )
    .await;
    assert_eq!(code, 403, "viewers do not write");
    let (code, _, _) = send(
        &app,
        "PUT",
        &format!("/v1/projects/{pid}/members"),
        Some(&b),
        None,
        Some(&origin),
        &invite,
    )
    .await;
    assert_eq!(code, 403, "only owners share");

    // As an editor he writes; the last owner cannot be removed.
    let promote = json!({ "email": bob, "role": "editor" }).to_string();
    send(
        &app,
        "PUT",
        &format!("/v1/projects/{pid}/members"),
        Some(&a),
        None,
        Some(&origin),
        &promote,
    )
    .await;
    let (code, _, body) = send(
        &app,
        "POST",
        "/v1/commands/activity.update",
        Some(&b),
        None,
        Some(&origin),
        edit,
    )
    .await;
    assert_eq!(code, 200, "{body}");
    let (code, _, _) = send(
        &app,
        "DELETE",
        &format!("/v1/projects/{pid}/members/{alice}"),
        Some(&a),
        None,
        Some(&origin),
        "",
    )
    .await;
    assert_eq!(code, 422, "last owner stays");

    // A stranger's project stays invisible to Bob.
    let (_, _, body) = send(
        &app,
        "POST",
        "/v1/commands/sample.load",
        Some(&a),
        None,
        Some(&origin),
        r#"{"name":"turnaround"}"#,
    )
    .await;
    let other = serde_json::from_str::<Value>(&body).unwrap()["project"]
        .as_str()
        .unwrap()
        .to_owned();
    let (code, _, _) = send(
        &app,
        "POST",
        "/v1/commands/schedule.run",
        Some(&b),
        None,
        Some(&origin),
        &json!({ "project": other }).to_string(),
    )
    .await;
    assert_eq!(code, 404);
    let (code, _, _) = send(
        &app,
        "GET",
        &format!("/v1/projects/{other}/members"),
        Some(&b),
        None,
        None,
        "",
    )
    .await;
    assert_eq!(code, 404);

    // Desktop sign-in: browser consent, loopback code, PKCE exchange.
    let verifier = "v".repeat(50);
    let challenge = pkce_challenge(&verifier);
    let start = format!("/auth/desktop?port=50123&state=abcdefgh12&challenge={challenge}");
    let (code, h, _) = send(&app, "GET", &start, None, None, None, "").await;
    assert_eq!(code, 303);
    assert!(
        h["location"]
            .to_str()
            .unwrap()
            .starts_with("/login?next=%2Fauth%2Fdesktop")
    );
    let (code, _, page) = send(&app, "GET", &start, Some(&b), None, None, "").await;
    assert_eq!(code, 200);
    assert!(
        page.contains("/auth/desktop/allow") && page.contains(&bob),
        "{page}"
    );
    let form = format!("port=50123&state=abcdefgh12&challenge={challenge}");
    let (code, h, _) = send_form(&app, "/auth/desktop/allow", &b, &origin, &form).await;
    assert_eq!(code, 303);
    let to = h["location"].to_str().unwrap().to_owned();
    assert!(
        to.starts_with("http://127.0.0.1:50123/callback?code="),
        "{to}"
    );
    assert!(to.ends_with("&state=abcdefgh12"));
    let code_value = to.split("code=").nth(1).unwrap().split('&').next().unwrap();
    let wrong = json!({ "code": code_value, "verifier": "w".repeat(50) }).to_string();
    let exchange =
        json!({ "code": code_value, "verifier": verifier, "device": "laptop" }).to_string();
    let (code, _, _) = send(
        &app,
        "POST",
        "/auth/desktop/token",
        None,
        None,
        None,
        &wrong,
    )
    .await;
    assert_eq!(code, 400, "wrong verifier");
    let (code, _, _) = send(
        &app,
        "POST",
        "/auth/desktop/token",
        None,
        None,
        None,
        &exchange,
    )
    .await;
    assert_eq!(code, 400, "the code was used up by the failed attempt");
    // A fresh round succeeds; the token acts as Bob (a person, not an agent).
    let (_, h, _) = send_form(&app, "/auth/desktop/allow", &b, &origin, &form).await;
    let to = h["location"].to_str().unwrap().to_owned();
    let code_value = to.split("code=").nth(1).unwrap().split('&').next().unwrap();
    let exchange =
        json!({ "code": code_value, "verifier": verifier, "device": "laptop" }).to_string();
    let (code, _, body) = send(
        &app,
        "POST",
        "/auth/desktop/token",
        None,
        None,
        None,
        &exchange,
    )
    .await;
    assert_eq!(code, 200, "{body}");
    let token = serde_json::from_str::<Value>(&body).unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let (code, _, me) = send(&app, "GET", "/v1/me", None, Some(&token), None, "").await;
    assert_eq!(code, 200);
    assert!(me.contains("\"desktop\"") && me.contains(&bob), "{me}");
    let (code, _, body) = send(
        &app,
        "POST",
        "/v1/commands/activity.update",
        None,
        Some(&token),
        None,
        edit,
    )
    .await;
    assert_eq!(code, 200, "the desktop app writes as the person: {body}");
    let init =
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {} }).to_string();
    let (code, _, _) = send(&app, "POST", "/mcp", None, Some(&token), None, &init).await;
    assert_eq!(code, 401, "desktop tokens are not agent tokens");
    let (code, _, _) = send(
        &app,
        "PUT",
        "/v1/me/settings",
        None,
        Some(&token),
        None,
        r#"{"theme":"dark"}"#,
    )
    .await;
    assert_eq!(code, 200);
    let (_, _, me) = send(&app, "GET", "/v1/me", None, Some(&token), None, "").await;
    assert!(me.contains("\"theme\":\"dark\""), "{me}");
}

/// Review fixes (H1, H6, M1, M2, M18): viewers and read-only tokens change
/// nothing, a server keeps its projects, a taken id is never handed to a
/// new owner, a project keeps an owner, and a token can revoke itself.
#[tokio::test(flavor = "multi_thread")]
async fn sync_06_roles_tokens_and_ownership() {
    let Some((app, state, origin, alice)) = test_app().await else {
        return;
    };
    let bob = format!("bob-{}@other.test", uuid::Uuid::now_v7());
    let a = session_for(&state, &alice).await;
    let b = session_for(&state, &bob).await;
    let post = |who: &str, cmd: &str, body: Value| {
        let (app, who, origin, cmd) = (app.clone(), who.to_owned(), origin.clone(), cmd.to_owned());
        async move {
            send(
                &app,
                "POST",
                &format!("/v1/commands/{cmd}"),
                Some(&who),
                None,
                Some(&origin),
                &body.to_string(),
            )
            .await
        }
    };
    let (_, _, body) = post(&a, "sample.load", json!({ "name": "bridge" })).await;
    let pid = serde_json::from_str::<Value>(&body).unwrap()["project"]
        .as_str()
        .unwrap()
        .to_owned();
    let (code, _, body) = post(
        &a,
        "sandbox.create",
        json!({ "project": pid, "name": "plan-b" }),
    )
    .await;
    assert_eq!(code, 200, "{body}");
    let invite = json!({ "email": bob, "role": "viewer" }).to_string();
    let members = format!("/v1/projects/{pid}/members");
    let (code, _, _) = send(
        &app,
        "PUT",
        &members,
        Some(&a),
        None,
        Some(&origin),
        &invite,
    )
    .await;
    assert_eq!(code, 200);

    // H1: a viewer neither discards someone's sandbox nor makes new ones.
    let (code, _, _) = post(
        &b,
        "sandbox.discard",
        json!({ "project": pid, "sandbox": "plan-b" }),
    )
    .await;
    assert_eq!(code, 403, "viewers do not discard");
    let (code, _, _) = post(
        &b,
        "sandbox.create",
        json!({ "project": pid, "name": "spam" }),
    )
    .await;
    assert_eq!(code, 403, "viewers do not create sandboxes");
    // H2 / M1: client-side sync commands and closing are not for a server.
    let (code, _, _) = post(&a, "project.close", json!({ "project": pid })).await;
    assert_eq!(code, 403, "a server keeps its projects");
    let (code, _, _) = post(&a, "sync.apply", json!({ "project": pid })).await;
    assert_eq!(code, 403, "sync.apply is the client's");

    // H6: a read-only token creates nothing and shares nothing.
    let secret = format!("{TOKEN_PREFIX}{}", oidc::random_token(32).unwrap());
    let (store, hash, e) = (
        state.store.clone(),
        oidc::hash_secret(&secret),
        alice.clone(),
    );
    tokio::task::spawn_blocking(move || {
        store.create_token(uuid::Uuid::now_v7(), &hash, &e, "reader", "read")
    })
    .await
    .unwrap()
    .unwrap();
    let (code, _, _) = send(
        &app,
        "POST",
        "/v1/commands/sample.load",
        None,
        Some(&secret),
        None,
        r#"{"name":"bridge"}"#,
    )
    .await;
    assert_eq!(code, 403, "read tokens create no projects");
    let (code, _, _) = send(&app, "PUT", &members, None, Some(&secret), None, &invite).await;
    assert_eq!(code, 403, "read tokens share nothing");
    let (code, _, _) = send(
        &app,
        "POST",
        "/v1/commands/schedule.run",
        None,
        Some(&secret),
        None,
        &json!({ "project": pid }).to_string(),
    )
    .await;
    assert_eq!(code, 200, "but they read");
    // M18: a token revokes itself and stops working.
    let (code, _, _) = send(
        &app,
        "DELETE",
        "/v1/me/token",
        None,
        Some(&secret),
        None,
        "",
    )
    .await;
    assert_eq!(code, 200);
    let (code, _, _) = send(&app, "GET", "/v1/me", None, Some(&secret), None, "").await;
    assert_eq!(code, 401, "revoked");

    // M1: an id that is taken is never handed to someone else.
    let (store, id, who) = (
        state.store.clone(),
        pid.parse::<uuid::Uuid>().unwrap(),
        bob.clone(),
    );
    let taken = tokio::task::spawn_blocking(move || store.claim_project(id, &who))
        .await
        .unwrap();
    assert!(taken.is_err(), "a project with members cannot be claimed");

    // M2: a sole owner cannot leave their own project.
    let (_, _, body) = post(&b, "sample.load", json!({ "name": "turnaround" })).await;
    let mine = serde_json::from_str::<Value>(&body).unwrap()["project"]
        .as_str()
        .unwrap()
        .to_owned();
    let (code, _, body) = send(
        &app,
        "DELETE",
        &format!("/v1/projects/{mine}/members/{bob}"),
        Some(&b),
        None,
        Some(&origin),
        "",
    )
    .await;
    assert_eq!(code, 422, "the last owner stays: {body}");
}

#[test]
fn sync_04_presence_follows_watch_streams() {
    let p = Arc::new(cloud::Presence::new());
    let pid = prswt_core::ids::ProjectId::new();
    let mut rx = p.changes.as_ref().unwrap().subscribe();
    // M3: one account holds at most 8 streams.
    let many: Vec<_> = (0..8).map(|_| p.join(pid, "c@x.test").unwrap()).collect();
    assert!(
        p.join(pid, "c@x.test").is_none(),
        "a ninth stream is refused"
    );
    drop(many);
    while rx.try_recv().is_ok() {}
    let a = p.join(pid, "a@x.test").unwrap();
    let b = p.join(pid, "b@x.test").unwrap();
    let a2 = p.join(pid, "a@x.test").unwrap();
    assert_eq!(p.people(pid), ["a@x.test", "b@x.test"]);
    drop(a);
    assert_eq!(
        p.people(pid),
        ["a@x.test", "b@x.test"],
        "a is still there twice"
    );
    drop(a2);
    drop(b);
    assert!(p.people(pid).is_empty());
    let mut seen = 0;
    while rx.try_recv().is_ok() {
        seen += 1;
    }
    assert_eq!(seen, 6, "each arrival and departure is announced");
    assert_eq!(after_login(Some("//evil.example")), "/");
    assert_eq!(after_login(Some("https://evil.example")), "/");
    assert_eq!(
        after_login(Some("/auth/desktop?port=1")),
        "/auth/desktop?port=1"
    );
}

/// The router over the test database, signed-in origin and an allowed email.
async fn test_app() -> Option<(Router, AppState, String, String)> {
    let url = std::env::var("PRSWT_TEST_DATABASE_URL").ok()?;
    assert!(
        url.rsplit('/').next().unwrap_or("").contains("test"),
        "test database only"
    );
    let store = tokio::task::spawn_blocking(move || Store::connect(&url))
        .await
        .unwrap()
        .unwrap();
    let email = format!("alice-{}@example.test", uuid::Uuid::now_v7());
    let cfg = Config {
        public_url: "http://localhost:18080".into(),
        bind: "127.0.0.1:0".parse().unwrap(),
        allowed: HashSet::from([email.clone()]),
        allowed_domains: HashSet::new(),
        providers: Vec::new(),
        session_hours: 1,
        dev_login: Some(email.clone()),
        tls: None,
    };
    let store = Arc::new(store);
    let mut ws = Workspace::new(Arc::new(prswt_app::FixedClock(
        "2027-02-01T08:00".parse().unwrap(),
    )));
    ws.on_create = Some(owner_hook(store.clone()));
    let workspace = Arc::new(Mutex::new(ws));
    let mut api = prswt_api::ApiState::new(workspace.clone());
    api.header_identity = false;
    let state = AppState {
        cfg: Arc::new(cfg),
        store,
        pending: Arc::new(PendingLogins::default()),
        http: reqwest::Client::new(),
        mcp: prswt_mcp::Shared {
            workspace,
            after_write: None,
        },
        events: api.events.clone(),
        presence: Arc::new(cloud::Presence::new()),
        desktop_codes: Arc::new(cloud::DesktopCodes::default()),
    };
    Some((
        app(state.clone(), api),
        state,
        "http://localhost:18080".into(),
        email,
    ))
}

/// A session cookie for `email`, made directly in the store.
async fn session_for(state: &AppState, email: &str) -> String {
    let secret = oidc::random_token(32).unwrap();
    let (store, hash, e) = (
        state.store.clone(),
        oidc::hash_secret(&secret),
        email.to_owned(),
    );
    tokio::task::spawn_blocking(move || store.create_session(&hash, &e, "test", 1))
        .await
        .unwrap()
        .unwrap();
    format!("{COOKIE}={secret}")
}

async fn send(
    app: &Router,
    method: &str,
    uri: &str,
    cookie: Option<&str>,
    token: Option<&str>,
    origin: Option<&str>,
    body: &str,
) -> (u16, HeaderMap, String) {
    let mut r = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(c) = cookie {
        r = r.header("cookie", c);
    }
    if let Some(t) = token {
        r = r.header("authorization", format!("Bearer {t}"));
    }
    if let Some(o) = origin {
        r = r.header("origin", o);
    }
    let resp = app
        .clone()
        .oneshot(r.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let code = resp.status().as_u16();
    let headers = resp.headers().clone();
    let bytes = to_bytes(resp.into_body(), 1 << 22).await.unwrap();
    (code, headers, String::from_utf8_lossy(&bytes).into_owned())
}

async fn send_form(
    app: &Router,
    uri: &str,
    cookie: &str,
    origin: &str,
    body: &str,
) -> (u16, HeaderMap, String) {
    let r = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("cookie", cookie)
        .header("origin", origin)
        .body(Body::from(body.to_owned()))
        .unwrap();
    let resp = app.clone().oneshot(r).await.unwrap();
    let code = resp.status().as_u16();
    let headers = resp.headers().clone();
    let bytes = to_bytes(resp.into_body(), 1 << 22).await.unwrap();
    (code, headers, String::from_utf8_lossy(&bytes).into_owned())
}

/// SRV-02: with a certificate the server speaks only TLS, and a client that
/// trusts that certificate's issuer gets through with verification on.
#[tokio::test]
async fn srv_02_tls_listener_verifies_and_refuses_plain_http() {
    // A private CA signs the server certificate, as Cloudflare's Origin CA would.
    let ca_key = rcgen::KeyPair::generate().unwrap();
    let mut ca_params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    // Distinct names: with rcgen's shared default DN the leaf looks self-issued.
    ca_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "PRSWT test CA");
    let ca_cert = ca_params.self_signed(&ca_key).unwrap();
    let issuer = rcgen::Issuer::new(ca_params, ca_key);
    let leaf_key = rcgen::KeyPair::generate().unwrap();
    let mut leaf_params = rcgen::CertificateParams::new(vec!["localhost".into()]).unwrap();
    leaf_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "localhost");
    let leaf = leaf_params.signed_by(&leaf_key, &issuer).unwrap();
    let dir = std::env::temp_dir().join(format!("prswt-tls-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let (cert, key) = (dir.join("cert.pem"), dir.join("key.pem"));
    std::fs::write(&cert, leaf.pem()).unwrap();
    std::fs::write(&key, leaf_key.serialize_pem()).unwrap();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let router = Router::new().route("/healthz", axum::routing::get(|| async { "ok" }));
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(serve(listener, Some((cert, key)), router, async {
        let _ = rx.await;
    }));
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let trusted = reqwest::Client::builder()
        .add_root_certificate(reqwest::Certificate::from_pem(ca_cert.pem().as_bytes()).unwrap())
        .build()
        .unwrap();
    let r = trusted
        .get(format!("https://localhost:{port}/healthz"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.text().await.unwrap(), "ok");

    let untrusted = reqwest::Client::new();
    assert!(
        untrusted
            .get(format!("https://localhost:{port}/healthz"))
            .send()
            .await
            .is_err(),
        "unknown issuer must fail"
    );
    assert!(
        untrusted
            .get(format!("http://127.0.0.1:{port}/healthz"))
            .send()
            .await
            .map_or(true, |r| !r.status().is_success()),
        "plain http must not be served"
    );

    let _ = tx.send(());
    let _ = server.await;
    let _ = std::fs::remove_dir_all(dir);
}

/// SRV-02: a renewed certificate file is picked up without a restart.
#[tokio::test]
async fn srv_02_certificate_reload_after_renewal() {
    let make = |cn: &str| {
        let key = rcgen::KeyPair::generate().unwrap();
        let mut p = rcgen::CertificateParams::new(vec!["localhost".into()]).unwrap();
        p.distinguished_name.push(rcgen::DnType::CommonName, cn);
        (p.self_signed(&key).unwrap().pem(), key.serialize_pem())
    };
    let dir = std::env::temp_dir().join(format!("prswt-reload-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let (cert, key) = (dir.join("cert.pem"), dir.join("key.pem"));
    let (c1, k1) = make("first");
    std::fs::write(&cert, &c1).unwrap();
    std::fs::write(&key, &k1).unwrap();
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let config = axum_server::tls_rustls::RustlsConfig::from_pem_file(&cert, &key)
        .await
        .unwrap();
    let before = config.get_inner();
    let watcher = tokio::spawn(watch_certificate(
        config.clone(),
        cert.clone(),
        key.clone(),
        std::time::Duration::from_millis(50),
    ));
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    assert!(
        std::sync::Arc::ptr_eq(&before, &config.get_inner()),
        "unchanged file: no reload"
    );

    // A broken renewal keeps the old certificate.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    std::fs::write(&cert, "not a certificate").unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(
        std::sync::Arc::ptr_eq(&before, &config.get_inner()),
        "bad file: old cert kept"
    );

    let (c2, k2) = make("second");
    std::fs::write(&key, &k2).unwrap();
    std::fs::write(&cert, &c2).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        !std::sync::Arc::ptr_eq(&before, &config.get_inner()),
        "renewed file: reloaded"
    );

    watcher.abort();
    let _ = std::fs::remove_dir_all(dir);
}
