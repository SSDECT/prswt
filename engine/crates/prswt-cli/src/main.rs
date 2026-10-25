//! `prswt-cli` — headless entry point (CLI-01, MCP-12).
//!
//! ```text
//! prswt-cli mcp      [--sample NAME] [--db] [--connect PORT] [--allow-agent-commit]
//! prswt-cli run      [--sample NAME] [--db] [--as human|agent] --cmd 'id={json}' [--cmd …]
//! prswt-cli serve    --control PORT [--sample NAME] [--db]
//! prswt-cli api      --control PORT [--sample NAME]   (HTTP on 127.0.0.1:PORT)
//! prswt-cli commands [SEARCH]
//! prswt-cli info     --db
//! prswt-cli import-xer FILE [--db]
//! prswt-cli export-xer FILE [--sample NAME | --db]
//! prswt-cli import-mspdi FILE [--db]
//! prswt-cli export-mspdi FILE [--sample NAME | --db]
//! ```
//! `--db` persists to PostgreSQL using `DATABASE_URL` (from the environment
//! or the nearest `.env`; Bitwarden later). Everything prints JSON on stdout;
//! diagnostics go to stderr.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::io::{BufReader, IsTerminal};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

use prswt_app::{AgentPolicy, Clock, Ctx, Workspace};
use prswt_core::{EngineError, Instant};
use prswt_mcp::{Headless, Remote, Server, control};
use prswt_store::Store;
use serde_json::{Value, json};

/// Wall clock for interactive use. The engine never reads time itself;
/// hosts like this one inject it (UTC; projects are single-time-zone).
struct SystemClock;

impl Clock for SystemClock {
    #[allow(clippy::disallowed_methods)]
    fn now(&self) -> Instant {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        // 2000-01-01T00:00Z is 946,684,800 s after the Unix epoch.
        Instant((i64::try_from(secs).unwrap_or(0) - 946_684_800) / 60)
    }
}

/// Parsed command-line flags shared by all subcommands.
#[derive(Default)]
struct Flags {
    sample: Option<String>,
    db: bool,
    connect: Option<u16>,
    control: Option<u16>,
    allow_agent_commit: bool,
    as_agent: bool,
    cmds: Vec<String>,
    rest: Vec<String>,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((sub, tail)) = args.split_first() else {
        eprintln!("usage: prswt-cli <mcp|run|serve|commands|info> [flags]");
        return ExitCode::FAILURE;
    };
    let flags = match parse(tail) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("prswt-cli: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = match sub.as_str() {
        "mcp" => mcp(&flags),
        "run" => run(&flags),
        "serve" => serve(&flags),
        "api" => api(&flags),
        "commands" => commands(&flags),
        "info" => info(),
        "import-xer" => import_file(&flags, "file.import_xer"),
        "export-xer" => export_file(&flags, "file.export_xer"),
        "import-mspdi" => import_file(&flags, "file.import_mspdi"),
        "export-mspdi" => export_file(&flags, "file.export_mspdi"),
        other => Err(EngineError::validation(format!(
            "unknown subcommand `{other}`"
        ))),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("prswt-cli: {:?}: {}", e.code, e.message);
            ExitCode::FAILURE
        }
    }
}

fn parse(args: &[String]) -> Result<Flags, String> {
    let mut f = Flags::default();
    let mut it = args.iter();
    let port = |v: Option<&String>| {
        v.and_then(|s| s.parse::<u16>().ok())
            .ok_or("expected a port number")
    };
    while let Some(a) = it.next() {
        match a.as_str() {
            "--sample" => f.sample = Some(it.next().ok_or("--sample needs a name")?.clone()),
            "--db" => f.db = true,
            "--connect" => f.connect = Some(port(it.next())?),
            "--control" => f.control = Some(port(it.next())?),
            "--allow-agent-commit" => f.allow_agent_commit = true,
            "--as" => f.as_agent = it.next().is_some_and(|s| s == "agent"),
            "--cmd" => f
                .cmds
                .push(it.next().ok_or("--cmd needs id={json}")?.clone()),
            other => f.rest.push(other.to_owned()),
        }
    }
    Ok(f)
}

/// Builds a workspace from flags: optional DB load, optional sample.
fn workspace(flags: &Flags) -> Result<(Workspace, Option<Store>), EngineError> {
    let mut ws = Workspace::new(Arc::new(SystemClock));
    if flags.allow_agent_commit {
        ws.policy = AgentPolicy::Allow;
    }
    let store = if flags.db { Some(open_store()?) } else { None };
    if let Some(s) = &store {
        for repo in s.load_all()? {
            ws.insert_repo(repo);
        }
    }
    if let Some(name) = &flags.sample {
        let open = ws.projects().any(|r| {
            r.branch(r.live)
                .is_ok_and(|b| b.head.project.name.to_lowercase().contains(name))
        });
        if !open {
            ws.execute(&Ctx::human("cli"), "sample.load", &json!({ "name": name }))?;
            // Persist right away: later writes only sync on their own events,
            // and a sample nobody edits would otherwise never be stored.
            if let Some(s) = &store {
                for repo in ws.projects() {
                    s.sync(repo)?;
                }
            }
        }
    }
    Ok((ws, store))
}

fn open_store() -> Result<Store, EngineError> {
    if std::env::var("DATABASE_URL").is_err()
        && let Some(url) = dotenv_value("DATABASE_URL")
    {
        return Store::connect(&url);
    }
    Store::from_env()
}

/// Reads `KEY=value` from the nearest `.env` (cwd or its parents).
fn dotenv_value(key: &str) -> Option<String> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let file = dir.join(".env");
        if let Ok(text) = std::fs::read_to_string(&file) {
            let prefix = format!("{key}=");
            if let Some(v) = text.lines().find_map(|l| l.strip_prefix(prefix.as_str())) {
                return Some(v.trim().to_owned());
            }
        }
        if !dir.pop() {
            return None;
        }
    }
}

fn mcp(flags: &Flags) -> Result<(), EngineError> {
    let io = |e: std::io::Error| EngineError::new(prswt_core::ErrorCode::Internal, e.to_string());
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        eprintln!(
            "prswt MCP server on stdio (JSON-RPC lines). Register with: claude mcp add prswt -- prswt-cli mcp --sample bridge"
        );
    }
    if let Some(port) = flags.connect {
        let mut server = Server::new(Remote::connect(port)?);
        return server
            .serve(BufReader::new(stdin.lock()), std::io::stdout().lock())
            .map_err(io);
    }
    let (ws, store) = workspace(flags)?;
    let mut server = Server::new(Headless {
        workspace: ws,
        store,
    });
    server
        .serve(BufReader::new(stdin.lock()), std::io::stdout().lock())
        .map_err(io)
}

fn run(flags: &Flags) -> Result<(), EngineError> {
    let (mut ws, store) = workspace(flags)?;
    let ctx = if flags.as_agent {
        Ctx::agent("cli")
    } else {
        Ctx::human("cli")
    };
    let mut results = Vec::new();
    for spec in &flags.cmds {
        let (id, params) = match spec.split_once('=') {
            Some((id, json)) => (
                id.trim(),
                serde_json::from_str::<Value>(json)
                    .map_err(|e| EngineError::validation(format!("--cmd {id}: bad JSON: {e}")))?,
            ),
            None => (spec.trim(), json!({})),
        };
        let out = ws.execute(&ctx, id, &params)?;
        results.push(json!({ "command": id, "result": out }));
    }
    if let Some(s) = &store {
        for repo in ws.projects() {
            s.sync(repo)?;
        }
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&results).unwrap_or_default()
    );
    Ok(())
}

fn serve(flags: &Flags) -> Result<(), EngineError> {
    let port = flags
        .control
        .ok_or_else(|| EngineError::validation("serve needs --control PORT"))?;
    let (ws, _store) = workspace(flags)?;
    eprintln!("control channel on 127.0.0.1:{port}");
    control::serve(Arc::new(Mutex::new(ws)), port)
        .map_err(|e| EngineError::new(prswt_core::ErrorCode::Internal, e.to_string()))
}

fn commands(flags: &Flags) -> Result<(), EngineError> {
    let mut ws = Workspace::new(Arc::new(SystemClock));
    let search = flags.rest.first().cloned();
    let out = ws.execute(
        &Ctx::human("cli"),
        "commands.list",
        &json!({ "search": search }),
    )?;
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    Ok(())
}

fn info() -> Result<(), EngineError> {
    let store = open_store()?;
    let ids = store.project_ids()?;
    let mut rows = Vec::new();
    for id in ids {
        let repo = store.load(id)?;
        let live = repo.branch(repo.live)?;
        rows.push(json!({
            "project": id, "name": live.head.project.name, "version": live.meta.version,
            "branches": repo.branches().count(), "activities": live.head.activities.len(),
        }));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({ "stored_projects": rows })).unwrap_or_default()
    );
    Ok(())
}

/// Reads a schedule file and imports it with `command` (humans only;
/// agents pass file text through MCP).
fn import_file(flags: &Flags, command: &str) -> Result<(), EngineError> {
    let path = flags
        .rest
        .first()
        .ok_or_else(|| EngineError::validation("import needs a file"))?;
    let text = std::fs::read_to_string(path)
        .map_err(|e| EngineError::validation(format!("{path}: {e}")))?;
    let (mut ws, store) = workspace(flags)?;
    let out = ws.execute(&Ctx::human("cli"), command, &json!({ "text": text }))?;
    if let Some(s) = &store {
        for repo in ws.projects() {
            s.sync(repo)?;
        }
    }
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    Ok(())
}

/// Exports the selected project with `command` and writes it to a file.
fn export_file(flags: &Flags, command: &str) -> Result<(), EngineError> {
    let path = flags
        .rest
        .first()
        .ok_or_else(|| EngineError::validation("export needs a file"))?;
    let (mut ws, _) = workspace(flags)?;
    let out = ws.execute(&Ctx::human("cli"), command, &json!({}))?;
    let text = out["text"].as_str().unwrap_or_default();
    std::fs::write(path, text).map_err(|e| EngineError::validation(format!("{path}: {e}")))?;
    println!("{}", json!({ "written": path, "bytes": text.len() }));
    Ok(())
}

/// Serves the HTTP API (`/v1/commands`, `/v1/events`) on the given port.
fn api(flags: &Flags) -> Result<(), EngineError> {
    let port = flags
        .control
        .ok_or_else(|| EngineError::validation("api needs --control PORT"))?;
    let (ws, _store) = workspace(flags)?;
    let state = prswt_api::ApiState::new(Arc::new(Mutex::new(ws)));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| EngineError::new(prswt_core::ErrorCode::Internal, e.to_string()))?;
    eprintln!("HTTP API on http://127.0.0.1:{port}/v1/commands");
    rt.block_on(prswt_api::serve(state, port))
        .map_err(|e| EngineError::new(prswt_core::ErrorCode::Internal, e.to_string()))
}
