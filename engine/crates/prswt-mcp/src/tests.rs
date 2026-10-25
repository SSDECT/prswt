//! MCP-11: protocol-level tests through `handle_line`, plus a real TCP
//! round-trip through the control channel (`Remote` backend).

use std::sync::{Arc, Mutex};

use prswt_app::{FixedClock, Workspace};
use serde_json::{Value, json};

use super::*;

#[test]
fn mcp_01_initialize_and_version_negotiation() {
    let mut s = server();
    let r = call(
        &mut s,
        "initialize",
        json!({ "protocolVersion": "2024-11-05", "clientInfo": { "name": "test-agent" } }),
    );
    assert_eq!(r["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(r["result"]["serverInfo"]["name"], "prswt");
    let r = call(
        &mut s,
        "initialize",
        json!({ "protocolVersion": "1999-01-01" }),
    );
    assert_eq!(
        r["result"]["protocolVersion"], PROTOCOL,
        "unknown versions get ours"
    );
    assert!(
        s.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .is_none()
    );
}

#[test]
fn mcp_11_jsonrpc_errors() {
    let mut s = server();
    let parse: Value = serde_json::from_str(&s.handle_line("{not json").unwrap()).unwrap();
    assert_eq!(parse["error"]["code"], -32700);
    assert_eq!(call(&mut s, "nope", json!({}))["error"]["code"], -32601);
    let bad = call(
        &mut s,
        "tools/call",
        json!({ "name": "load_sample", "arguments": { "name": 5 } }),
    );
    assert_eq!(bad["error"]["code"], -32602);
    let missing = call(
        &mut s,
        "tools/call",
        json!({ "name": "realize_risk", "arguments": {} }),
    );
    assert_eq!(missing["error"]["code"], -32602);
    let unknown_arg = call(
        &mut s,
        "tools/call",
        json!({ "name": "list_projects", "arguments": { "x": 1 } }),
    );
    assert_eq!(unknown_arg["error"]["code"], -32602);
    assert!(s.handle_line("").is_none());
    // Regression (system test S07): non-object messages must be answered.
    for bad in ["[]", "[1,2]", "42", "\"x\"", "{\"jsonrpc\":\"2.0\"}"] {
        let r: Value = serde_json::from_str(&s.handle_line(bad).expect(bad)).unwrap();
        assert_eq!(r["error"]["code"], -32600, "{bad}");
    }
}

#[test]
fn mcp_03_09_agent_workflow_over_tools() {
    let mut s = server();
    let tools = call(&mut s, "tools/list", json!({}));
    assert!(tools["result"]["tools"].as_array().unwrap().len() > 35);
    tool(&mut s, "load_sample", json!({ "name": "bridge" }));
    let summary = tool(&mut s, "get_schedule_summary", json!({}));
    assert_eq!(summary["project"], "Bridge Rehabilitation");
    // Agents cannot write live …
    let denied = call(
        &mut s,
        "tools/call",
        json!({ "name": "update_activity", "arguments": { "activity": "C30", "duration": "30d" } }),
    );
    assert_eq!(denied["result"]["isError"], true);
    assert_eq!(
        denied["result"]["structuredContent"]["error"]["code"],
        "LIVE_WRITE_FORBIDDEN"
    );
    // … but can in a sandbox.
    tool(&mut s, "create_sandbox", json!({ "name": "audit" }));
    tool(
        &mut s,
        "realize_risk",
        json!({ "branch": "audit", "risk": "R3" }),
    );
    let sim = tool(
        &mut s,
        "run_impact_simulation",
        json!({ "branch": "audit", "iterations": 300, "compare_to": "live" }),
    );
    assert!(sim["delta_vs"]["finish_days"]["mean"].as_f64().unwrap() > 0.0);
    let generic = tool(
        &mut s,
        "execute_command",
        json!({ "command": "schedule.critical_path", "params": { "branch": "audit", "k": 2 } }),
    );
    assert_eq!(generic["paths"].as_array().unwrap().len(), 2);
}

#[test]
fn mcp_02_resources_and_prompts() {
    let mut s = server();
    tool(&mut s, "load_sample", json!({ "name": "turnaround" }));
    let list = call(&mut s, "resources/list", json!({}));
    let uris: Vec<&str> = list["result"]["resources"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|r| r["uri"].as_str())
        .collect();
    assert!(uris.contains(&"prswt://projects/Unit%202%20Turnaround/b/live/summary"));
    for uri in uris {
        let r = call(&mut s, "resources/read", json!({ "uri": uri }));
        assert!(r.get("result").is_some(), "{uri}: {r}");
    }
    let a = call(
        &mut s,
        "resources/read",
        json!({ "uri": "prswt://projects/Unit%202%20Turnaround/b/live/activities/M30" }),
    );
    assert!(
        a["result"]["contents"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Column tray repairs")
    );
    assert_eq!(
        call(&mut s, "resources/read", json!({ "uri": "prswt://nope" }))["error"]["code"],
        -32002
    );
    assert!(
        !call(&mut s, "resources/templates/list", json!({}))["result"]["resourceTemplates"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let prompts = call(&mut s, "prompts/list", json!({}));
    assert_eq!(prompts["result"]["prompts"].as_array().unwrap().len(), 5);
    let p = call(
        &mut s,
        "prompts/get",
        json!({ "name": "supply_chain_correlator", "arguments": { "event": "port strike" } }),
    );
    assert!(
        p["result"]["messages"][0]["content"]["text"]
            .as_str()
            .unwrap()
            .contains("port strike")
    );
}

#[test]
fn mcp_11_every_tool_survives_empty_arguments() {
    let mut s = server();
    tool(&mut s, "load_sample", json!({ "name": "bridge" }));
    for t in tools::TOOLS {
        let r = call(
            &mut s,
            "tools/call",
            json!({ "name": t.name, "arguments": {} }),
        );
        let internal = r["result"]["structuredContent"]["error"]["code"] == "INTERNAL";
        assert!(!internal, "{} crashed: {r}", t.name);
    }
}

#[test]
fn mcp_11_remote_backend_over_control_channel() {
    let ws = Arc::new(Mutex::new(Workspace::new(Arc::new(FixedClock(
        "2027-02-01T08:00".parse().unwrap(),
    )))));
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let shared = ws.clone();
    std::thread::spawn(move || control::serve(shared, port));
    let mut remote = None;
    for _ in 0..100 {
        if let Ok(r) = Remote::connect(port) {
            remote = Some(r);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let mut s = Server::new(remote.unwrap());
    tool(&mut s, "load_sample", json!({ "name": "bridge" }));
    let summary = tool(&mut s, "get_schedule_summary", json!({}));
    assert_eq!(summary["activities"], 14);
    assert_eq!(
        ws.lock().unwrap().projects().count(),
        1,
        "the app's workspace changed"
    );
    let bad = control::handle(&ws, "{\"method\":\"x\"}");
    assert!(bad.get("error").is_some());
}

#[test]
fn mcp_13_fixed_identity_and_shared_backend() {
    let ws = Arc::new(Mutex::new(Workspace::new(Arc::new(FixedClock(
        "2027-02-01T08:00".parse().unwrap(),
    )))));
    let saved = Arc::new(Mutex::new(0usize));
    let counter = saved.clone();
    let shared = Shared {
        workspace: ws.clone(),
        after_write: Some(Arc::new(move |_| {
            *counter.lock().unwrap() += 1;
            Ok(())
        })),
    };
    let mut s = Server::with_identity(shared, prswt_app::Ctx::agent("token:laptop"));
    call(
        &mut s,
        "initialize",
        json!({ "clientInfo": { "name": "pretend-human" } }),
    );
    tool(&mut s, "load_sample", json!({ "name": "bridge" }));
    assert_eq!(*saved.lock().unwrap(), 1, "persisted after the write");
    let denied = call(
        &mut s,
        "tools/call",
        json!({ "name": "update_activity", "arguments": { "activity": "C30", "duration": "9d" } }),
    );
    assert_eq!(
        denied["result"]["structuredContent"]["error"]["code"], "LIVE_WRITE_FORBIDDEN",
        "still an agent"
    );
    let hist = tool(
        &mut s,
        "execute_command",
        json!({ "command": "project.history", "params": {} }),
    );
    assert!(
        hist.to_string().contains("token:laptop") || hist.to_string().contains("system"),
        "{hist}"
    );
    assert_eq!(ws.lock().unwrap().projects().count(), 1);
}

fn server() -> Server<Headless> {
    Server::new(Headless::new(Arc::new(FixedClock(
        "2027-02-01T08:00".parse().unwrap(),
    ))))
}

fn call<B: Backend>(s: &mut Server<B>, method: &str, params: Value) -> Value {
    let req = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
    serde_json::from_str(&s.handle_line(&req.to_string()).unwrap()).unwrap()
}

fn tool<B: Backend>(s: &mut Server<B>, name: &str, args: Value) -> Value {
    let r = call(s, "tools/call", json!({ "name": name, "arguments": args }));
    assert_eq!(r["result"]["isError"], false, "{name}: {r}");
    r["result"]["structuredContent"].clone()
}
