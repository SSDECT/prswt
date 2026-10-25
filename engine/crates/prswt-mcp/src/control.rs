//! Localhost control channel (GridCraft's `--control <port>` pattern).
//!
//! JSON lines over TCP on `127.0.0.1` only. A request is
//! `{"id":1,"method":"engine.execute","params":{"command":"schedule.run","params":{},"agent":"name"?}}`
//! and the reply is `{"id":1,"result":…}` or `{"id":1,"error":EngineError}`.
//! It lets an MCP server (`prswt-cli mcp --connect <port>`) drive the same
//! workspace a human is looking at in the desktop app.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use prswt_app::{Ctx, Workspace};
use prswt_core::{EngineError, ErrorCode};
use serde_json::{Value, json};

/// Longest request line accepted from a client (hostile-input guard).
const MAX_LINE: usize = 4 * 1024 * 1024;

/// Serves the shared workspace until the listener fails. Each client gets a
/// thread; commands run one at a time under the workspace lock.
pub fn serve(workspace: Arc<Mutex<Workspace>>, port: u16) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    for stream in listener.incoming() {
        let stream = stream?;
        let ws = workspace.clone();
        std::thread::spawn(move || {
            let _ = client(&ws, stream);
        });
    }
    Ok(())
}

/// Handles one client connection until it disconnects.
fn client(ws: &Arc<Mutex<Workspace>>, stream: TcpStream) -> std::io::Result<()> {
    let mut writer = stream.try_clone()?;
    for line in BufReader::new(stream).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let reply = handle(ws, &line);
        writeln!(writer, "{reply}")?;
        writer.flush()?;
    }
    Ok(())
}

/// Parses and executes one request line; never panics on bad input.
pub fn handle(ws: &Arc<Mutex<Workspace>>, line: &str) -> Value {
    if line.len() > MAX_LINE {
        return json!({ "id": null, "error": EngineError::new(ErrorCode::Limit, "request too large") });
    }
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return json!({ "id": null, "error": EngineError::validation(format!("bad JSON: {e}")) });
        }
    };
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    if req.get("method").and_then(Value::as_str) != Some("engine.execute") {
        return json!({ "id": id, "error": EngineError::not_found("method (only engine.execute)") });
    }
    let p = req.get("params").cloned().unwrap_or(Value::Null);
    let Some(command) = p.get("command").and_then(Value::as_str) else {
        return json!({ "id": id, "error": EngineError::validation("missing params.command") });
    };
    // Remote MCP clients are agents and keep the agent policy; a human UI
    // talks to the workspace directly, not through this channel.
    let ctx = match p.get("agent").and_then(Value::as_str) {
        Some(name) => Ctx::agent(name),
        None => Ctx::agent("control-channel"),
    };
    let params = p.get("params").cloned().unwrap_or(Value::Null);
    let Ok(mut guard) = ws.lock() else {
        return json!({ "id": id, "error": EngineError::new(ErrorCode::Internal, "workspace unavailable") });
    };
    match guard.execute(&ctx, command, &params) {
        Ok(r) => json!({ "id": id, "result": r }),
        Err(e) => json!({ "id": id, "error": e }),
    }
}
