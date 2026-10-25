//! SYNC-01..04 on the desktop (ADR-0010): the network side of cloud
//! projects. A worker thread signs in through the browser (loopback
//! redirect + PKCE), calls the server, keeps local copies in sync and
//! listens for live changes; the window talks to it through channels and
//! never blocks on the network.
//!
//! Sync itself is the engine's `sync.*` protocol: pull (`sync.events` on
//! the server, `sync.apply` here), then push (`sync.outbox` here,
//! `sync.receive` there), retrying when someone else wrote first. Local
//! copies are cached as `.prswt` files with their sync state, so cloud
//! projects open and change offline and catch up later.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use prswt_app::{Ctx, Workspace};
use serde_json::{Value, json};
use sha2::Digest;

/// Default server of cloud projects.
pub const DEFAULT_SERVER: &str = "https://prswt.juz.ai";
/// Keychain service name under which the desktop token is kept.
const KEYRING_SERVICE: &str = "PRSWT";
/// Polls of the loopback listener (each 200 ms) before sign-in gives up: 5 minutes.
const SIGN_IN_POLLS: u32 = 1_500;
/// Push attempts per sync before giving up (someone keeps writing first).
const SYNC_ROUNDS: usize = 4;
/// Largest response body read from the server (project files included).
const MAX_BODY: u64 = 256 * 1024 * 1024;
/// Silence on a live-update stream before it counts as dropped (the
/// server sends a keep-alive every 15 s).
const WATCH_SILENCE: Duration = Duration::from_secs(45);

/// Work for the cloud thread.
#[derive(Debug, Clone)]
pub enum Job {
    /// Sign in through the browser.
    SignIn,
    /// The browser sign-in (run on its own thread) produced a token.
    Token(String),
    /// Use a token kept from an earlier run (start-up).
    Resume,
    /// Forget the token.
    SignOut,
    /// List cloud projects.
    List,
    /// Upload a local project (id); it becomes a cloud project.
    Upload(String),
    /// Open a cloud project (id): cached copy if any, else download.
    Open(String),
    /// Sync a cloud project (id).
    Sync(String),
    /// Members of a project.
    Members(String),
    /// Add or change a member: (project, email, role).
    Invite(String, String, String),
    /// Remove a member: (project, email).
    Remove(String, String),
    /// Save the account settings.
    Settings(Value),
    /// Stop the live-update stream of a project (id): it was closed.
    Unwatch(String),
}

/// What the cloud thread reports back.
#[derive(Debug, Clone)]
pub enum Done {
    /// Signed in (or resumed) as this email, with saved settings.
    SignedIn(String, Value),
    /// No account on this computer any more.
    SignedOut,
    /// Waiting for something (shown with a spinner); `None` = finished.
    Busy(Option<String>),
    /// Cloud projects: `[{project, name, role}]`.
    Projects(Vec<Value>),
    /// A project is now open locally as a cloud copy (id).
    Opened(String),
    /// A sync finished: (project, branches still unsent, conflicts, the
    /// project's revision when it was sent). A newer revision means edits
    /// made during the sync, still to send.
    Synced(String, u64, Vec<Value>, Option<u64>),
    /// A sync could not reach the server: (project, why).
    Offline(String, String),
    /// Members: (project, `[(email, role)]`).
    Members(String, Vec<(String, String)>),
    /// Who has the project open: (project, emails).
    Present(String, Vec<String>),
    /// Someone changed the project on the server: (project, who).
    Changed(String, String),
    /// Something failed; shown to the person.
    Error(String),
}

/// The window's handle on the cloud thread.
#[derive(Debug)]
pub struct Client {
    jobs: Sender<Job>,
    /// Reports from the cloud thread.
    pub done: Receiver<Done>,
}

impl Client {
    /// Starts the cloud thread for `server`, working on `ws`; local copies
    /// are cached in `cache`.
    pub fn start(server: String, ws: Arc<Mutex<Workspace>>, cache: PathBuf) -> Self {
        let (jobs, rx) = channel::<Job>();
        let (tx, done) = channel::<Done>();
        let worker = Worker {
            server,
            token: None,
            email: None,
            ws,
            cache,
            out: tx,
            jobs: jobs.clone(),
            watching: Arc::default(),
            signing_in: Arc::default(),
        };
        std::thread::spawn(move || worker.run(&rx));
        Self { jobs, done }
    }

    /// Queues a job (ignored if the thread is gone).
    pub fn send(&self, job: Job) {
        let _ = self.jobs.send(job);
    }
}

/// State of the cloud thread.
struct Worker {
    server: String,
    token: Option<String>,
    email: Option<String>,
    ws: Arc<Mutex<Workspace>>,
    cache: PathBuf,
    out: Sender<Done>,
    jobs: Sender<Job>,
    /// Projects with a live-update stream running, each with its stop flag
    /// (a stream that ends by itself removes its entry).
    watching: Watching,
    /// A browser sign-in is waiting (one at a time).
    signing_in: Arc<AtomicBool>,
}

/// Live-update streams by project id, with their stop flags.
type Watching = Arc<Mutex<HashMap<String, Arc<AtomicBool>>>>;

impl Worker {
    fn run(mut self, rx: &Receiver<Job>) {
        while let Ok(job) = rx.recv() {
            // Several queued syncs of one project collapse into one.
            let mut batch = vec![job];
            while let Ok(more) = rx.try_recv() {
                batch.push(more);
            }
            let mut seen = Vec::new();
            for job in batch {
                if let Job::Sync(id) = &job {
                    if seen.contains(id) {
                        continue;
                    }
                    seen.push(id.clone());
                }
                if let Err(e) = self.handle(job.clone()) {
                    match job {
                        // Only an existing cloud copy is "offline": a failed
                        // upload or download leaves a local project local.
                        Job::Sync(id) if e.offline => {
                            self.report(Done::Offline(id, e.message));
                        }
                        _ => self.report(Done::Error(e.message)),
                    }
                }
            }
        }
    }

    fn report(&self, d: Done) {
        let _ = self.out.send(d);
    }

    fn handle(&mut self, job: Job) -> Result<(), Failure> {
        match job {
            Job::SignIn => self.sign_in(),
            Job::Token(token) => {
                if keyring::Entry::new(KEYRING_SERVICE, &self.server)
                    .and_then(|e| e.set_password(&token))
                    .is_err()
                {
                    self.report(Done::Error(
                        "signed in, but the system keychain refused the token: sign in again next time"
                            .into(),
                    ));
                }
                self.token = Some(token);
                self.resume()
            }
            Job::Resume => self.resume(),
            Job::SignOut => {
                if std::env::var_os("PRSWT_TOKEN").is_some() {
                    return Err(Failure::local(
                        "this session uses PRSWT_TOKEN: unset it to sign out",
                    ));
                }
                // The token stops working on the server too (a copy left
                // behind is useless); offline, it stays revocable on /account.
                let _ = self.http("DELETE", "/v1/me/token", None);
                if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, &self.server) {
                    let _ = entry.delete_credential();
                }
                self.stop_watching(None);
                self.token = None;
                self.email = None;
                self.report(Done::SignedOut);
                Ok(())
            }
            Job::Unwatch(id) => {
                self.stop_watching(Some(&id));
                Ok(())
            }
            Job::List => {
                let v = self.command("project.list", &json!({ "brief": true }))?;
                let me = self.get("/v1/me")?;
                let role = |id: &str| {
                    me["projects"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .find(|p| p["project"] == id)
                        .and_then(|p| p["role"].as_str())
                        .unwrap_or("viewer")
                        .to_owned()
                };
                let rows = v["projects"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|p| {
                        let id = p["project"].as_str().unwrap_or("");
                        json!({ "project": id, "name": p["name"], "role": role(id) })
                    })
                    .collect();
                self.report(Done::Projects(rows));
                Ok(())
            }
            Job::Upload(id) => self.upload(&id),
            Job::Open(id) => self.open(&id),
            Job::Sync(id) => self.sync(&id),
            Job::Members(id) => self.members(&id),
            Job::Invite(id, email, role) => {
                self.http(
                    "PUT",
                    &format!("/v1/projects/{id}/members"),
                    Some(&json!({ "email": email, "role": role })),
                )?;
                self.members(&id)
            }
            Job::Remove(id, email) => {
                self.http(
                    "DELETE",
                    &format!("/v1/projects/{id}/members/{}", enc(&email)),
                    None,
                )?;
                self.members(&id)
            }
            Job::Settings(v) => {
                self.http("PUT", "/v1/me/settings", Some(&v))?;
                Ok(())
            }
        }
    }

    // ── sign-in ──

    /// RFC 8252: a one-shot listener on 127.0.0.1 receives the browser's
    /// redirect with a code; the code plus the PKCE verifier buys a token.
    /// The wait runs on its own thread, so syncs go on meanwhile; the token
    /// comes back as [`Job::Token`].
    fn sign_in(&mut self) -> Result<(), Failure> {
        if self.signing_in.swap(true, Ordering::SeqCst) {
            return Err(Failure::local("sign-in is already waiting for the browser"));
        }
        let listener = match TcpListener::bind("127.0.0.1:0") {
            Ok(l) => l,
            Err(e) => {
                self.signing_in.store(false, Ordering::SeqCst);
                return Err(Failure::local(e));
            }
        };
        let (server, out, jobs, busy) = (
            self.server.clone(),
            self.out.clone(),
            self.jobs.clone(),
            self.signing_in.clone(),
        );
        std::thread::spawn(move || {
            let result = browser_sign_in(&server, &listener, &out);
            let _ = out.send(Done::Busy(None));
            busy.store(false, Ordering::SeqCst);
            match result {
                Ok(token) => {
                    let _ = jobs.send(Job::Token(token));
                }
                Err(e) => {
                    let _ = out.send(Done::Error(e.message));
                }
            }
        });
        Ok(())
    }

    /// Picks up the kept token (if any) and checks it with the server.
    /// `PRSWT_TOKEN` overrides the keychain (automation, tests).
    fn resume(&mut self) -> Result<(), Failure> {
        if self.token.is_none() {
            self.token = std::env::var("PRSWT_TOKEN")
                .ok()
                .filter(|t| t.starts_with("prswt_"));
        }
        if self.token.is_none() {
            self.token = keyring::Entry::new(KEYRING_SERVICE, &self.server)
                .ok()
                .and_then(|e| e.get_password().ok());
        }
        if self.token.is_none() {
            // An account remembered from an earlier run is not signed in.
            self.report(Done::SignedOut);
            return Ok(());
        }
        match self.get("/v1/me") {
            Ok(me) => {
                let email = me["email"].as_str().unwrap_or("").to_owned();
                self.email = Some(email.clone());
                self.report(Done::SignedIn(email, me["settings"].clone()));
                Ok(())
            }
            Err(e) if e.unauthorized => {
                // Revoked on the account page: forget it.
                self.token = None;
                if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, &self.server) {
                    let _ = entry.delete_credential();
                }
                self.stop_watching(None);
                self.report(Done::SignedOut);
                Ok(())
            }
            Err(e) => Err(e),
        }
    }
}

/// The browser half of [`Worker::sign_in`]: opens the consent page, waits
/// for the code on `listener` and trades it (with the PKCE verifier) for a
/// token.
fn browser_sign_in(
    server: &str,
    listener: &TcpListener,
    out: &Sender<Done>,
) -> Result<String, Failure> {
    let port = listener.local_addr().map_err(Failure::local)?.port();
    let state = random(16)?;
    let verifier = random(32)?;
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(sha2::Sha256::digest(verifier.as_bytes()));
    let url = format!("{server}/auth/desktop?port={port}&state={state}&challenge={challenge}");
    if open::that_detached(&url).is_err() {
        return Err(Failure::local(format!(
            "open this address in a browser: {url}"
        )));
    }
    let _ = out.send(Done::Busy(Some("Waiting for the browser…".into())));
    let code = wait_for_code(listener, &state)?;
    let device = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default();
    let reply = reply(
        client()?
            .post(format!("{server}/auth/desktop/token"))
            .json(&json!({ "code": code, "verifier": verifier, "device": device }))
            .send(),
    )?;
    reply["token"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| Failure::local("the server sent no token"))
}

impl Worker {
    // ── projects ──

    fn upload(&mut self, id: &str) -> Result<(), Failure> {
        let text = self.local("file.save_project", &json!({ "project": id }))?["text"]
            .as_str()
            .unwrap_or("")
            .to_owned();
        match self.command("file.open_project", &json!({ "text": text })) {
            Ok(_) => {}
            // It is one of this person's cloud projects already.
            Err(e) if e.message.starts_with("this project is already open") => {
                return Err(Failure::local(
                    "this project is already in the cloud: use File ▸ Cloud ▸ Open from cloud",
                ));
            }
            // Someone else's project with this id: the server says so.
            Err(e) => return Err(e),
        }
        self.local(
            "sync.link",
            &json!({ "project": id, "server": self.server }),
        )?;
        let rev = self.revision(id);
        self.write_cache(id)?;
        self.watch(id);
        self.report(Done::Opened(id.to_owned()));
        self.report(Done::Synced(id.to_owned(), 0, Vec::new(), rev));
        Ok(())
    }

    fn open(&mut self, id: &str) -> Result<(), Failure> {
        let status = self.local("sync.status", &json!({ "project": id }));
        let open_here = status.as_ref().is_ok_and(|v| v["linked"] == true);
        if status.is_ok() && !open_here {
            // Open here as a plain file: that copy becomes the cloud copy
            // (only if it is the same as the server's, else it would
            // count as in sync while it is not).
            return Err(Failure::local(
                "this project is open as a local file: close it, then open it from the cloud",
            ));
        }
        if !open_here {
            // A cached copy of this server is used; a missing, unreadable
            // or other server's copy is replaced by a fresh download.
            let cached = std::fs::read_to_string(self.cache_path(id))
                .ok()
                .filter(|t| {
                    serde_json::from_str::<Value>(t)
                        .is_ok_and(|v| v["sync"]["server"].as_str() == Some(self.server.as_str()))
                });
            let opened = match cached {
                Some(text) => self
                    .local(
                        "file.open_project",
                        &json!({ "text": text, "keep_sync": true }),
                    )
                    .ok()
                    .filter(|r| r["linked"] == true),
                None => None,
            };
            if opened.is_none() {
                let v = self.command("file.save_project", &json!({ "project": id }))?;
                let text = v["text"].as_str().unwrap_or("").to_owned();
                if self.local("project.list", &json!({ "brief": true }))?["projects"]
                    .as_array()
                    .is_some_and(|ps| ps.iter().any(|p| p["project"] == id))
                {
                    // A cache that opened without its link: drop it first.
                    self.local("project.close", &json!({ "project": id }))?;
                }
                self.local("file.open_project", &json!({ "text": text }))?;
                self.local(
                    "sync.link",
                    &json!({ "project": id, "server": self.server }),
                )?;
            }
        }
        self.report(Done::Opened(id.to_owned()));
        self.watch(id);
        self.sync(id)
    }

    /// Pull, rebase, push; repeat while someone else writes first. The
    /// cache is written whatever happens (unsent work survives a crash).
    fn sync(&mut self, id: &str) -> Result<(), Failure> {
        let linked = self
            .local("sync.status", &json!({ "project": id }))
            .is_ok_and(|v| v["linked"] == true);
        if !linked {
            return Ok(());
        }
        let result = self.sync_inner(id);
        if let Err(e) = self.write_cache(id) {
            self.report(Done::Error(format!(
                "could not keep the local copy: {}",
                e.message
            )));
        }
        if result.is_ok() {
            // Opened while signed out, or the stream ended: watch again.
            self.watch(id);
        }
        result
    }

    fn sync_inner(&mut self, id: &str) -> Result<(), Failure> {
        let mut conflicts = Vec::new();
        for _ in 0..SYNC_ROUNDS {
            let st = self.local("sync.status", &json!({ "project": id }))?;
            if st["linked"] != true {
                return Ok(());
            }
            let since: serde_json::Map<String, Value> = st["branches"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|b| !b["synced"].is_null())
                .filter_map(|b| Some((b["id"].as_str()?.to_owned(), b["synced"].clone())))
                .collect();
            let pulled = self.command("sync.events", &json!({ "project": id, "since": since }))?;
            let applied = self.local(
                "sync.apply",
                &json!({ "project": id, "branches": pulled["branches"] }),
            )?;
            conflicts.extend(applied["conflicts"].as_array().cloned().unwrap_or_default());
            match self.push(id) {
                Ok(()) => {
                    let pending =
                        self.local("sync.status", &json!({ "project": id }))?["pending_branches"]
                            .as_u64()
                            .unwrap_or(0);
                    let rev = self.revision(id);
                    self.report(Done::Synced(id.to_owned(), pending, conflicts, rev));
                    return Ok(());
                }
                Err(e) if e.conflict => {}
                Err(e) => return Err(e),
            }
        }
        Err(Failure::local(
            "the project kept changing on the server; try again",
        ))
    }

    fn push(&mut self, id: &str) -> Result<(), Failure> {
        let out = self.local("sync.outbox", &json!({ "project": id }))?;
        for row in out["branches"].as_array().cloned().unwrap_or_default() {
            let mut req = row;
            req["project"] = json!(id);
            let reply = self.command("sync.receive", &req)?;
            self.local(
                "sync.apply",
                &json!({ "project": id, "branches": [{ "meta": reply["meta"], "events": reply["events"] }] }),
            )?;
        }
        Ok(())
    }

    fn members(&mut self, id: &str) -> Result<(), Failure> {
        let v = self.get(&format!("/v1/projects/{id}/members"))?;
        let list = v["members"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| {
                Some((
                    m["email"].as_str()?.to_owned(),
                    m["role"].as_str()?.to_owned(),
                ))
            })
            .collect();
        self.report(Done::Members(id.to_owned(), list));
        let present = v["present"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| e.as_str().map(str::to_owned))
            .collect();
        self.report(Done::Present(id.to_owned(), present));
        Ok(())
    }

    /// Starts the live-update stream of a project (once).
    fn watch(&mut self, id: &str) {
        let Some(token) = self.token.clone() else {
            return;
        };
        let stop = Arc::new(AtomicBool::new(false));
        {
            let Ok(mut w) = self.watching.lock() else {
                return;
            };
            if w.contains_key(id) {
                return;
            }
            w.insert(id.to_owned(), stop.clone());
        }
        let stream = Stream {
            server: self.server.clone(),
            token,
            id: id.to_owned(),
            me: self.email.clone().unwrap_or_default(),
            out: self.out.clone(),
            jobs: self.jobs.clone(),
            stop: stop.clone(),
        };
        let watching = self.watching.clone();
        std::thread::spawn(move || {
            stream.run();
            // Ended by itself (revoked, removed): let a later sync restart it.
            if let Ok(mut w) = watching.lock()
                && w.get(&stream.id).is_some_and(|s| Arc::ptr_eq(s, &stop))
            {
                w.remove(&stream.id);
            }
        });
    }

    /// Stops the stream of project `id`, or of every project.
    fn stop_watching(&mut self, id: Option<&str>) {
        let Ok(mut w) = self.watching.lock() else {
            return;
        };
        match id {
            Some(id) => {
                if let Some(stop) = w.remove(id) {
                    stop.store(true, Ordering::SeqCst);
                }
            }
            None => {
                for (_, stop) in w.drain() {
                    stop.store(true, Ordering::SeqCst);
                }
            }
        }
    }

    /// The change counter of a local project (as the window sees it).
    fn revision(&self, id: &str) -> Option<u64> {
        self.local("project.list", &json!({ "brief": true })).ok()?["projects"]
            .as_array()?
            .iter()
            .find(|p| p["project"] == id)?["revision"]
            .as_u64()
    }

    // ── local engine and cache ──

    /// Runs a command on the local workspace as the signed-in person.
    fn local(&self, command: &str, params: &Value) -> Result<Value, Failure> {
        let ctx = Ctx::human(self.email.as_deref().unwrap_or("desktop"));
        let mut ws = self
            .ws
            .lock()
            .map_err(|_| Failure::local("workspace unavailable"))?;
        ws.execute(&ctx, command, params)
            .map_err(|e| Failure::local(e.message))
    }

    fn cache_path(&self, id: &str) -> PathBuf {
        let safe: String = id
            .chars()
            .filter(|c| c.is_ascii_hexdigit() || *c == '-')
            .collect();
        self.cache.join(format!("{safe}.prswt"))
    }

    /// Writes the local copy and its sync state to the cache folder.
    fn write_cache(&self, id: &str) -> Result<(), Failure> {
        let text = self.local(
            "file.save_project",
            &json!({ "project": id, "with_sync": true }),
        )?["text"]
            .as_str()
            .unwrap_or("")
            .to_owned();
        std::fs::create_dir_all(&self.cache).map_err(Failure::local)?;
        crate::files::write_atomic(&self.cache_path(id), &text).map_err(Failure::local)
    }

    // ── HTTP ──

    /// Runs an engine command on the server (`/v1/commands/{id}`).
    fn command(&self, id: &str, params: &Value) -> Result<Value, Failure> {
        self.http("POST", &format!("/v1/commands/{id}"), Some(params))
    }

    fn get(&self, path: &str) -> Result<Value, Failure> {
        self.http("GET", path, None)
    }

    fn http(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, Failure> {
        let token = self
            .token
            .as_deref()
            .ok_or_else(|| Failure::local("sign in first (File ▸ Cloud ▸ Sign in)"))?;
        let client = client()?;
        let url = format!("{}{path}", self.server);
        let req = match method {
            "GET" => client.get(&url),
            "PUT" => client.put(&url),
            "DELETE" => client.delete(&url),
            _ => client.post(&url),
        }
        .bearer_auth(token);
        let req = match body {
            Some(b) => req.json(b),
            None => req,
        };
        reply(req.send())
    }
}

/// Why a cloud action failed, and how the window should treat it.
#[derive(Debug)]
struct Failure {
    message: String,
    /// The server could not be reached (keep working locally).
    offline: bool,
    /// The server refused because someone wrote first (pull and retry).
    conflict: bool,
    /// The token is not accepted any more.
    unauthorized: bool,
}

impl Failure {
    fn local(e: impl std::fmt::Display) -> Self {
        Self {
            message: e.to_string(),
            offline: false,
            conflict: false,
            unauthorized: false,
        }
    }
}

fn client() -> Result<reqwest::blocking::Client, Failure> {
    reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .user_agent(concat!("prswt-desktop/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(Failure::local)
}

/// JSON of a successful reply; failures sorted into offline (no network,
/// or a gateway/unavailable reply), conflict, signed-out and the rest
/// (with the server's message, e.g. a 500).
fn reply(r: reqwest::Result<reqwest::blocking::Response>) -> Result<Value, Failure> {
    let resp = r.map_err(|e| Failure {
        message: format!("cannot reach the server ({e})"),
        offline: true,
        conflict: false,
        unauthorized: false,
    })?;
    let status = resp.status();
    let mut text = String::new();
    resp.take(MAX_BODY)
        .read_to_string(&mut text)
        .map_err(Failure::local)?;
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::String(text));
    if status.is_success() {
        return Ok(v);
    }
    let message = v["error"]["message"]
        .as_str()
        .or_else(|| v["error"].as_str())
        .or_else(|| v.as_str())
        .unwrap_or("request failed")
        .to_owned();
    Err(Failure {
        message: format!("{message} ({})", status.as_u16()),
        offline: matches!(status.as_u16(), 502..=504),
        conflict: status.as_u16() == 409,
        unauthorized: status.as_u16() == 401,
    })
}

/// Accepts the browser's redirect on the loopback listener and returns its
/// code (checking `state`); answers the browser with a short page. Other
/// requests (a stray local process, a browser's favicon fetch) get 400 and
/// the wait goes on; a connection that sends nothing is dropped after 5 s.
fn wait_for_code(listener: &TcpListener, state: &str) -> Result<String, Failure> {
    listener.set_nonblocking(true).map_err(Failure::local)?;
    for _ in 0..SIGN_IN_POLLS {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                let mut line = String::new();
                let _ = BufReader::new(&stream).take(8 * 1024).read_line(&mut line);
                let query = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|p| p.split_once('?'))
                    .map(|(_, q)| q.to_owned())
                    .unwrap_or_default();
                let param = |k: &str| {
                    query
                        .split('&')
                        .filter_map(|kv| kv.split_once('='))
                        .find(|(a, _)| *a == k)
                        .map(|(_, v)| v.to_owned())
                };
                let ok = param("state").as_deref() == Some(state) && param("code").is_some();
                let (status, page) = if ok {
                    (
                        "200 OK",
                        "Signed in. You can close this tab and return to PRSWT.",
                    )
                } else {
                    ("400 Bad Request", "This is not a PRSWT sign-in.")
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n\
                     <!doctype html><meta charset=utf-8><title>PRSWT</title><p style=\"font:16px system-ui;margin:3em\">{page}</p>"
                );
                if ok {
                    return param("code").ok_or_else(|| Failure::local("no code"));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(200));
            }
            Err(e) => return Err(Failure::local(e)),
        }
    }
    Err(Failure::local("sign-in timed out"))
}

/// One project's live-update stream, run on its own thread.
struct Stream {
    server: String,
    token: String,
    id: String,
    /// The signed-in email (own changes trigger no sync).
    me: String,
    out: Sender<Done>,
    jobs: Sender<Job>,
    /// Set when the project is closed or the person signs out.
    stop: Arc<AtomicBool>,
}

impl Stream {
    /// Reads the stream; reconnects after drops until stopped, signed out
    /// (401) or removed from the project (404). Changes by others trigger
    /// a sync; presence goes to the window.
    fn run(&self) {
        let Ok(http) = reqwest::blocking::Client::builder()
            .user_agent(concat!("prswt-desktop/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            // Per read: a stream that goes silent (dead network) is dropped.
            .timeout(WATCH_SILENCE)
            .build()
        else {
            return;
        };
        let mut failures = 0u32;
        while !self.stopped() {
            let resp = http
                .get(format!("{}/v1/projects/{}/watch", self.server, self.id))
                .bearer_auth(&self.token)
                .send();
            match resp {
                Ok(r) if r.status().is_success() => {
                    failures = 0;
                    if !self.read(r) {
                        return;
                    }
                }
                Ok(r) if matches!(r.status().as_u16(), 401 | 404) => return,
                _ => {}
            }
            failures = failures.saturating_add(1);
            // Back off up to a minute between reconnects, waking to stop.
            for _ in 0..u64::from(failures.min(12)) * 5 {
                if self.stopped() {
                    return;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// Handles one connection's events; `false` when the loop must end
    /// (stopped, or the window is gone).
    fn read(&self, r: reqwest::blocking::Response) -> bool {
        let (id, me, out, jobs) = (self.id.as_str(), self.me.as_str(), &self.out, &self.jobs);
        {
            {
                let mut event = String::new();
                for line in BufReader::new(r).lines() {
                    if self.stopped() {
                        return false;
                    }
                    let Ok(line) = line else { break };
                    if let Some(e) = line.strip_prefix("event:") {
                        e.trim().clone_into(&mut event);
                    } else if let Some(data) = line.strip_prefix("data:") {
                        let v: Value = serde_json::from_str(data.trim()).unwrap_or(Value::Null);
                        let sent = match event.as_str() {
                            "presence" => {
                                let people = v["people"]
                                    .as_array()
                                    .into_iter()
                                    .flatten()
                                    .filter_map(|p| p.as_str().map(str::to_owned))
                                    .collect();
                                out.send(Done::Present(id.to_owned(), people)).is_ok()
                            }
                            "change" => {
                                let who =
                                    v["author"]["name"].as_str().unwrap_or("someone").to_owned();
                                if who == me {
                                    true
                                } else {
                                    let _ = jobs.send(Job::Sync(id.to_owned()));
                                    out.send(Done::Changed(id.to_owned(), who)).is_ok()
                                }
                            }
                            _ => true,
                        };
                        if !sent {
                            return false; // the window is gone
                        }
                    }
                }
            }
        }
        true
    }
}

/// `n` random bytes, base64url without padding.
fn random(n: usize) -> Result<String, Failure> {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf).map_err(|e| Failure::local(format!("no randomness: {e}")))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf))
}

/// Percent-encodes a path segment.
fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~@".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
