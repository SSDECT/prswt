//! MCP server (Pillar III, ADR-0006 revised): newline-delimited JSON-RPC 2.0
//! over stdio, hand-written and synchronous so every message is testable
//! through [`Server::handle_line`]. Backends: in-process [`Headless`]
//! workspace (optionally persisted) or [`Remote`] — a running app's
//! localhost control channel ([`control`]).

pub mod control;
mod prompts;
mod resources;
pub mod tools;

use std::io::{BufRead, Write};
use std::net::TcpStream;
use std::sync::Arc;

use prswt_app::{Clock, Ctx, Workspace};
use prswt_core::{EngineError, ErrorCode};
use prswt_store::Store;
use serde_json::{Value, json};

/// Preferred MCP protocol version.
pub const PROTOCOL: &str = "2025-06-18";
const SUPPORTED: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
/// Longest JSON-RPC line accepted (hostile input).
const MAX_LINE: usize = 4 * 1024 * 1024;

/// What the MCP server drives.
pub trait Backend {
    /// Runs engine command `command` with `params` as `ctx`.
    fn call(&mut self, ctx: &Ctx, command: &str, params: &Value) -> Result<Value, EngineError>;
}

/// In-process engine, optionally persisted after every write.
pub struct Headless {
    /// The engine.
    pub workspace: Workspace,
    /// Persistence, when enabled.
    pub store: Option<Store>,
}

impl std::fmt::Debug for Headless {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Headless")
            .field("persisted", &self.store.is_some())
            .finish_non_exhaustive()
    }
}

impl Headless {
    /// In-memory backend without persistence.
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self {
            workspace: Workspace::new(clock),
            store: None,
        }
    }

    /// Loads every stored project and persists future writes.
    pub fn with_store(mut self, store: Store) -> Result<Self, EngineError> {
        for repo in store.load_all()? {
            self.workspace.insert_repo(repo);
        }
        self.store = Some(store);
        Ok(self)
    }
}

impl Backend for Headless {
    fn call(&mut self, ctx: &Ctx, command: &str, params: &Value) -> Result<Value, EngineError> {
        let out = self.workspace.execute(ctx, command, params)?;
        let writes = prswt_app::catalog()
            .iter()
            .any(|c| c.id == command && c.writes);
        if writes && let Some(store) = &self.store {
            for repo in self.workspace.projects() {
                store.sync(repo)?;
            }
        }
        Ok(out)
    }
}

/// Called after writes made through a [`Shared`] backend (e.g. persist).
pub type AfterWrite = Arc<dyn Fn(&Workspace) -> Result<(), EngineError> + Send + Sync>;

/// A workspace shared with other adapters (HTTP API, web UI), used by the
/// remote MCP endpoint. Cheap to clone.
#[derive(Clone)]
pub struct Shared {
    /// The shared engine.
    pub workspace: Arc<std::sync::Mutex<Workspace>>,
    /// Persistence hook after writes.
    pub after_write: Option<AfterWrite>,
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared").finish_non_exhaustive()
    }
}

impl Backend for Shared {
    fn call(&mut self, ctx: &Ctx, command: &str, params: &Value) -> Result<Value, EngineError> {
        let mut ws = self
            .workspace
            .lock()
            .map_err(|_| EngineError::new(ErrorCode::Internal, "workspace unavailable"))?;
        let out = ws.execute(ctx, command, params)?;
        let writes = prswt_app::catalog()
            .iter()
            .any(|c| c.id == command && c.writes);
        if writes && let Some(hook) = &self.after_write {
            hook(&ws)?;
        }
        Ok(out)
    }
}

/// A running desktop app reached through its control channel.
#[derive(Debug)]
pub struct Remote {
    reader: std::io::BufReader<TcpStream>,
    writer: TcpStream,
    next: u64,
}

impl Remote {
    /// Connects to a control channel on `127.0.0.1:port`.
    pub fn connect(port: u16) -> Result<Self, EngineError> {
        let stream = TcpStream::connect(("127.0.0.1", port)).map_err(|e| {
            EngineError::new(
                ErrorCode::NotFound,
                format!("control channel on port {port}: {e}"),
            )
        })?;
        let writer = stream
            .try_clone()
            .map_err(|e| EngineError::new(ErrorCode::Internal, e.to_string()))?;
        Ok(Self {
            reader: std::io::BufReader::new(stream),
            writer,
            next: 1,
        })
    }
}

impl Backend for Remote {
    fn call(&mut self, ctx: &Ctx, command: &str, params: &Value) -> Result<Value, EngineError> {
        let io = |e: std::io::Error| {
            EngineError::new(ErrorCode::Internal, format!("control channel: {e}"))
        };
        let id = self.next;
        self.next += 1;
        let agent = match &ctx.author {
            prswt_app::Who::Agent(n) => Some(n.clone()),
            _ => None,
        };
        let req = json!({ "id": id, "method": "engine.execute", "params": { "command": command, "params": params, "agent": agent } });
        writeln!(self.writer, "{req}").map_err(io)?;
        let mut line = String::new();
        self.reader.read_line(&mut line).map_err(io)?;
        let resp: Value = serde_json::from_str(&line).map_err(|e| {
            EngineError::new(ErrorCode::Internal, format!("control channel reply: {e}"))
        })?;
        match resp.get("error") {
            Some(err) => Err(serde_json::from_value(err.clone())
                .unwrap_or_else(|_| EngineError::new(ErrorCode::Internal, err.to_string()))),
            None => Ok(resp.get("result").cloned().unwrap_or(Value::Null)),
        }
    }
}

/// MCP JSON-RPC session.
pub struct Server<B: Backend> {
    backend: B,
    ctx: Ctx,
    /// Identity set by the host (token); `initialize` may not change it.
    fixed_identity: bool,
}

impl<B: Backend> std::fmt::Debug for Server<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Server")
            .field("ctx", &self.ctx)
            .finish_non_exhaustive()
    }
}

impl<B: Backend> Server<B> {
    /// Server over `backend`; callers act as an agent until `initialize` names them.
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            ctx: Ctx::agent("mcp-client"),
            fixed_identity: false,
        }
    }

    /// Server acting as `ctx` regardless of what the client claims (remote
    /// MCP: the identity comes from the bearer token).
    pub fn with_identity(backend: B, ctx: Ctx) -> Self {
        Self {
            backend,
            ctx,
            fixed_identity: true,
        }
    }

    /// The backend, for inspection.
    pub fn backend(&mut self) -> &mut B {
        &mut self.backend
    }

    /// Reads requests line by line until EOF; logs nothing to stdout.
    pub fn serve(&mut self, input: impl BufRead, mut output: impl Write) -> std::io::Result<()> {
        for line in input.lines() {
            let line = line?;
            if let Some(reply) = self.handle_line(&line) {
                writeln!(output, "{reply}")?;
                output.flush()?;
            }
        }
        Ok(())
    }

    /// One JSON-RPC message in, at most one reply out (`None` for notifications).
    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        if line.trim().is_empty() {
            return None;
        }
        if line.len() > MAX_LINE {
            return Some(rpc_error(&Value::Null, -32600, "message too large").to_string());
        }
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                return Some(
                    rpc_error(&Value::Null, -32700, &format!("parse error: {e}")).to_string(),
                );
            }
        };
        // Batches (arrays) and other non-objects are invalid in MCP; they
        // must still get an error reply, or the client waits forever.
        if !msg.is_object() {
            return Some(
                rpc_error(
                    &Value::Null,
                    -32600,
                    "invalid request: expected a JSON object (batches are not supported)",
                )
                .to_string(),
            );
        }
        let id = msg.get("id").cloned();
        let Some(method) = msg.get("method").and_then(Value::as_str) else {
            let id = id.unwrap_or(Value::Null);
            return Some(rpc_error(&id, -32600, "invalid request: missing method").to_string());
        };
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let result = self.dispatch(method, &params);
        let id = id?; // notifications get no reply
        Some(
            match result {
                Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
                Err((code, m)) => rpc_error(&id, code, &m),
            }
            .to_string(),
        )
    }

    fn dispatch(&mut self, method: &str, params: &Value) -> Result<Value, (i64, String)> {
        match method {
            "initialize" => {
                let asked = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or(PROTOCOL);
                let version = if SUPPORTED.contains(&asked) {
                    asked
                } else {
                    PROTOCOL
                };
                if let Some(name) = params
                    .pointer("/clientInfo/name")
                    .and_then(Value::as_str)
                    .filter(|_| !self.fixed_identity)
                {
                    self.ctx = Ctx::agent(&name.chars().take(64).collect::<String>());
                }
                Ok(json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false }, "resources": { "listChanged": false }, "prompts": { "listChanged": false } },
                    "serverInfo": { "name": "prswt", "title": "PRSWT project controls & risk engine", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": "PRSWT schedules projects (CPM, P6-style), levels resources, computes EVM and runs Monte Carlo risk analysis. \
                        Work in sandboxes: create_sandbox, make changes there, run_impact_simulation with compare_to=\"live\", diff_branches, \
                        then ask a human to commit. Durations are strings like \"5d\" or {o,m,p}. Read prswt://commands for every engine command.",
                }))
            }
            "ping" => Ok(json!({})),
            "notifications/initialized" | "notifications/cancelled" => Ok(Value::Null),
            "tools/list" => Ok(json!({ "tools": tool_list() })),
            "tools/call" => self.call_tool(params),
            "resources/list" => {
                Ok(json!({ "resources": resources::list(&mut self.backend, &self.ctx) }))
            }
            "resources/templates/list" => {
                Ok(json!({ "resourceTemplates": resources::templates() }))
            }
            "resources/read" => {
                let uri = params
                    .get("uri")
                    .and_then(Value::as_str)
                    .ok_or((-32602, "missing `uri`".to_owned()))?;
                let body = resources::read(&mut self.backend, &self.ctx, uri)
                    .map_err(|e| (-32002, e.message))?;
                Ok(
                    json!({ "contents": [{ "uri": uri, "mimeType": "application/json", "text": pretty(&body) }] }),
                )
            }
            "prompts/list" => Ok(json!({ "prompts": prompts::list() })),
            "prompts/get" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or((-32602, "missing `name`".to_owned()))?;
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                prompts::get(name, &args).ok_or((-32602, format!("unknown prompt `{name}`")))
            }
            other => Err((-32601, format!("method not found: {other}"))),
        }
    }

    fn call_tool(&mut self, params: &Value) -> Result<Value, (i64, String)> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or((-32602, "missing tool `name`".to_owned()))?;
        let args = params.get("arguments").cloned().unwrap_or(json!({}));
        let (command, args) = match name {
            "execute_command" => {
                let c = args
                    .get("command")
                    .and_then(Value::as_str)
                    .ok_or((-32602, "missing `command`".to_owned()))?;
                (
                    c.to_owned(),
                    args.get("params").cloned().unwrap_or(json!({})),
                )
            }
            "list_commands" => ("commands.list".to_owned(), args),
            _ => {
                let tool = tools::TOOLS
                    .iter()
                    .find(|t| t.name == name)
                    .ok_or((-32602, format!("unknown tool `{name}`")))?;
                tools::check(tool.params, &args).map_err(|m| (-32602, m))?;
                (tool.command.to_owned(), args)
            }
        };
        Ok(match self.backend.call(&self.ctx, &command, &args) {
            Ok(v) => {
                json!({ "content": [{ "type": "text", "text": pretty(&v) }], "structuredContent": wrap(v), "isError": false })
            }
            Err(e) => json!({
                "content": [{ "type": "text", "text": format!("{:?}: {}", e.code, e.message) }],
                "structuredContent": { "error": e },
                "isError": true,
            }),
        })
    }
}

fn tool_list() -> Vec<Value> {
    let mut out: Vec<Value> = tools::TOOLS
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": tools::schema(t.params),
                "annotations": { "readOnlyHint": t.read_only, "destructiveHint": false },
            })
        })
        .collect();
    out.push(json!({
        "name": "execute_command",
        "description": "Run any engine command by id (see list_commands / prswt://commands) with its params object.",
        "inputSchema": { "type": "object", "properties": { "command": { "type": "string" }, "params": { "type": "object" } }, "required": ["command"] },
    }));
    out.push(json!({
        "name": "list_commands",
        "description": "Engine command ids, labels, P6 menu paths and parameter docs.",
        "inputSchema": { "type": "object", "properties": { "search": { "type": "string" } } },
        "annotations": { "readOnlyHint": true },
    }));
    out
}

/// `structuredContent` must be an object.
fn wrap(v: Value) -> Value {
    if v.is_object() {
        v
    } else {
        json!({ "result": v })
    }
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

fn rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

#[cfg(test)]
mod tests;
