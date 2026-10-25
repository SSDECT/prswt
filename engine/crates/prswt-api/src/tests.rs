//! API tests through the router (no sockets).

use std::sync::{Arc, Mutex};

use axum::body::{Body, to_bytes};
use axum::http::Request;
use prswt_app::{FixedClock, Workspace};
use tower::ServiceExt;

use super::*;

#[tokio::test]
async fn api_01_commands_round_trip_and_status_codes() {
    let state = state();
    let app = router(state.clone());
    let (code, body) = call(&app, "GET", "/v1/commands", None, None).await;
    assert_eq!(code, 200);
    assert!(body["commands"].as_array().unwrap().len() > 40);

    let (code, _) = call(
        &app,
        "POST",
        "/v1/commands/sample.load",
        Some(json!({ "name": "bridge" })),
        None,
    )
    .await;
    assert_eq!(code, 200);
    let (code, body) = call(&app, "POST", "/v1/commands/schedule.run", None, None).await;
    assert_eq!(
        (code, body["project"].as_str()),
        (200, Some("Bridge Rehabilitation"))
    );

    let (code, body) = call(&app, "POST", "/v1/commands/nope", None, None).await;
    assert_eq!(
        (code, body["error"]["code"].as_str()),
        (404, Some("NOT_FOUND"))
    );
    let (code, _) = call(
        &app,
        "POST",
        "/v1/commands/activity.update",
        Some(json!({ "activity": "C30", "duration": "x" })),
        None,
    )
    .await;
    assert_eq!(code, 422);
    let (code, body) = call(
        &app,
        "POST",
        "/v1/commands/activity.update",
        Some(json!({ "activity": "C30", "duration": "9d" })),
        Some("bot"),
    )
    .await;
    assert_eq!(
        (code, body["error"]["code"].as_str()),
        (403, Some("LIVE_WRITE_FORBIDDEN"))
    );
}

#[tokio::test]
async fn api_02_writes_are_broadcast_as_events() {
    let state = state();
    let mut rx = state.events.subscribe();
    let app = router(state.clone());
    call(
        &app,
        "POST",
        "/v1/commands/sample.load",
        Some(json!({ "name": "turnaround" })),
        None,
    )
    .await;
    call(
        &app,
        "POST",
        "/v1/commands/activity.update",
        Some(json!({ "activity": "M30", "duration": "120h" })),
        None,
    )
    .await;
    let mut events = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }
    let authors: Vec<&str> = events
        .iter()
        .filter_map(|e| e["author"]["type"].as_str())
        .collect();
    assert_eq!(
        authors,
        ["system", "human"],
        "sample baseline, then the edit"
    );
    assert_eq!(events[1]["version"], 2);
}

fn state() -> ApiState {
    ApiState::new(Arc::new(Mutex::new(Workspace::new(Arc::new(FixedClock(
        "2027-02-01T08:00".parse().unwrap(),
    ))))))
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    agent: Option<&str>,
) -> (u16, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(a) = agent {
        req = req.header("x-prswt-agent", a);
    }
    let req = req
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let code = resp.status().as_u16();
    let bytes = to_bytes(resp.into_body(), 1 << 22).await.unwrap();
    (code, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}
