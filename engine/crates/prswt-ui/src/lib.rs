//! PRSWT desktop UI (UI-01..06, ADR-0008): a thin egui front end.
//!
//! Every action goes through an [`Engine`], in the desktop app the
//! in-process workspace (`LocalEngine`, feature `local`); tests can pass
//! their own. Either way it is a catalog command,
//! exactly like MCP and the CLI, so the UI is one more equal client of the
//! engine (GridCraft's "the UI is thin" rule). Views read JSON results and
//! cache them until something changes. Colours come from [`Tokens`].

// Drawing maps day counts and indices to f32 screen points; sub-pixel
// precision loss there is harmless, so these casts are allowed in this crate.
#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]

mod chrome;
mod cloud;
mod forms;
mod network;
mod table;
mod theme;

use std::collections::HashSet;
#[cfg(feature = "local")]
use std::sync::{Arc, Mutex};

use egui::{Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, Ui, Vec2};
#[cfg(feature = "local")]
use prswt_app::{Ctx, Workspace};
use serde_json::{Value, json};

pub use cloud::{CloudProject, CloudRequest, CloudView, ROLES, SyncState, role_label};
pub use theme::{SEMIBOLD, Tokens, apply_style, semibold};

/// Minutes in a day, for converting instants to timeline pixels.
const DAY_MINUTES: f32 = 1440.0;
/// Height of one Gantt row in points.
const ROW_H: f32 = 28.0;
/// Rows asked for per `schedule.activities` call (the command's own cap).
const ROW_PAGE: usize = 1_000;
/// Most activity rows the views load; bigger plans show the first ones.
const MAX_ROWS: usize = 50_000;
/// Gantt table columns: (title, width in points, alignment). The activity
/// column shows the code in small type before the name.
const GANTT_COLS: [(&str, f32, Align2); 5] = [
    ("ACTIVITY", 270.0, Align2::LEFT_CENTER),
    ("DUR", 50.0, Align2::RIGHT_CENTER),
    ("START", 82.0, Align2::RIGHT_CENTER),
    ("FINISH", 82.0, Align2::RIGHT_CENTER),
    ("FLOAT", 54.0, Align2::RIGHT_CENTER),
];
/// Gantt zoom presets: (label, points per day); "Fit" follows the window.
const ZOOMS: [(&str, f32); 3] = [("Day", 28.0), ("Week", 8.0), ("Month", 2.5)];
/// Month abbreviations for dates shown to people.
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Main views of the window, in navigation order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// Gantt chart: activity table and bars, grouped by WBS.
    Gantt,
    /// Editable activity table.
    Activities,
    /// Network (PERT) diagram: boxes by logic depth, linked by arrows.
    Network,
    /// Resource usage, cost and earned value.
    Resources,
    /// Risk register with one-click what-ifs.
    Risks,
    /// Monte Carlo results.
    Simulation,
    /// What-if branches: diff, commit, discard.
    Sandboxes,
}

impl Tab {
    /// All views in navigation order (Ctrl+1 … Ctrl+7).
    pub const ALL: [Tab; 7] = [
        Tab::Gantt,
        Tab::Activities,
        Tab::Network,
        Tab::Resources,
        Tab::Risks,
        Tab::Simulation,
        Tab::Sandboxes,
    ];

    /// View name in the navigation panel and the header.
    pub fn label(self) -> &'static str {
        match self {
            Tab::Gantt => "Schedule",
            Tab::Activities => "Activities",
            Tab::Network => "Network",
            Tab::Resources => "Resources & cost",
            Tab::Risks => "Risks",
            Tab::Simulation => "Risk analysis",
            Tab::Sandboxes => "What-if branches",
        }
    }

    /// The number key that opens the view with Ctrl (⌘ on macOS).
    pub fn key(self) -> egui::Key {
        use egui::Key;
        match self {
            Tab::Gantt => Key::Num1,
            Tab::Activities => Key::Num2,
            Tab::Network => Key::Num3,
            Tab::Resources => Key::Num4,
            Tab::Risks => Key::Num5,
            Tab::Simulation => Key::Num6,
            Tab::Sandboxes => Key::Num7,
        }
    }
}

/// Sample projects offered under File ▸ Open sample.
pub const SAMPLES: [&str; 2] = ["bridge", "turnaround"];

/// Exchange formats the File menu can import and export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Primavera P6 XER (`file.import_xer` / `file.export_xer`).
    Xer,
    /// Microsoft Project XML (`file.import_mspdi` / `file.export_mspdi`).
    Mspdi,
}

impl Format {
    /// Both formats, in menu order.
    pub const ALL: [Format; 2] = [Format::Xer, Format::Mspdi];

    /// Engine command that imports this format from text.
    pub fn import_command(self) -> &'static str {
        match self {
            Format::Xer => "file.import_xer",
            Format::Mspdi => "file.import_mspdi",
        }
    }

    /// Engine command that exports the selected branch as text.
    pub fn export_command(self) -> &'static str {
        match self {
            Format::Xer => "file.export_xer",
            Format::Mspdi => "file.export_mspdi",
        }
    }

    /// File extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Format::Xer => "xer",
            Format::Mspdi => "xml",
        }
    }

    /// Name shown in menus and file dialogs.
    pub fn label(self) -> &'static str {
        match self {
            Format::Xer => "Primavera P6 XER",
            Format::Mspdi => "Microsoft Project XML",
        }
    }
}

/// A File-menu action that needs the operating system (dialogs, disk).
///
/// The UI crate does no IO; it queues these and the desktop shell takes
/// them with [`PrswtApp::take_requests`] and answers through
/// [`PrswtApp::project_text`] and [`PrswtApp::open_text`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileRequest {
    /// File ▸ Open: pick a `.prswt` file.
    Open,
    /// File ▸ Open recent: this path.
    OpenRecent(String),
    /// File ▸ Save the selected project (asks for a path the first time).
    Save,
    /// File ▸ Save as: always asks for a path.
    SaveAs,
    /// File ▸ Import a foreign format.
    Import(Format),
    /// File ▸ Export the selected branch.
    Export(Format),
    /// File ▸ Close the selected project (the shell asks about unsaved changes).
    Close,
    /// File ▸ Quit.
    Quit,
    /// Save text the UI produced (CSV, a printable page) under a name the
    /// person picks; `open_after` opens it with the system viewer (the
    /// browser prints a page or saves it as PDF).
    SaveText {
        /// File-dialog title.
        title: String,
        /// Suggested file name, extension included.
        file_name: String,
        /// Extension without the dot (dialog filter).
        extension: String,
        /// The contents.
        text: String,
        /// Open the file once written.
        open_after: bool,
    },
}

/// Calendar presets of `project.create`: (id, label shown in the dialog).
const CALENDAR_PRESETS: [(&str, &str); 3] = [
    ("5x8", "Standard: Mon–Fri, 8 h/day"),
    ("6x10", "Extended: Mon–Sat, 10 h/day"),
    ("7x24", "Continuous: 7 days, 24 h"),
];

/// Inputs of the File ▸ New project dialog. Everything is picked, not
/// typed, except the name: a calendar for the date, lists for the rest.
#[derive(Debug, Clone)]
struct NewProject {
    name: String,
    date: jiff::civil::Date,
    hour: i8,
    calendar: &'static str,
}

impl NewProject {
    /// Defaults: today at 08:00 on the standard calendar.
    fn starting(today: jiff::civil::Date) -> Self {
        Self {
            name: "New project".into(),
            date: today,
            hour: 8,
            calendar: "5x8",
        }
    }

    /// Start in the engine's `YYYY-MM-DDTHH:MM` form.
    fn start(&self) -> String {
        format!("{}T{:02}:00", self.date, self.hour)
    }
}

/// A failed engine call (code + human message), backend-independent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineFailure {
    /// Machine-readable code (`NOT_FOUND`, `VALIDATION`, …).
    pub code: String,
    /// Human-readable explanation.
    pub message: String,
}

/// Something that runs catalog commands for the UI.
pub trait Engine {
    /// Runs `command` with `params` and returns its JSON result.
    fn exec(&mut self, command: &str, params: &Value) -> Result<Value, EngineFailure>;

    /// DSK-05: starts `command` on another thread so the window keeps
    /// repainting. `None` means this engine cannot, and the caller runs it
    /// inline with [`Engine::exec`].
    fn spawn(&mut self, _command: &str, _params: &Value) -> Option<Job> {
        None
    }

    /// Asks the background command to stop (no-op when unsupported).
    fn cancel(&mut self) {}
}

/// A command running in the background; poll it once per frame.
#[derive(Debug)]
pub struct Job {
    rx: std::sync::mpsc::Receiver<Result<Value, EngineFailure>>,
}

impl Job {
    /// The result once the command finished; `Some(Err)` if its thread died.
    fn poll(&self) -> Option<Result<Value, EngineFailure>> {
        match self.rx.try_recv() {
            Ok(r) => Some(r),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err(EngineFailure {
                code: "INTERNAL".into(),
                message: "background command stopped unexpectedly".into(),
            })),
        }
    }
}

/// Commands slow enough to run in the background (Monte Carlo, leveling).
pub const LONG_COMMANDS: [&str; 2] = ["simulation.run", "resource.level"];

/// A background command and what to do with its result.
#[derive(Debug)]
struct Running {
    command: String,
    job: Job,
    /// egui time (seconds) at the first busy frame, for the elapsed display.
    started: Option<f64>,
}

/// The in-process engine used by the desktop app.
#[cfg(feature = "local")]
pub struct LocalEngine {
    /// Shared workspace (the control channel may share it).
    pub ws: Arc<Mutex<Workspace>>,
    /// Identity the UI acts as.
    pub ctx: Ctx,
    /// Stop switch of the workspace, taken while it was unlocked.
    pub cancel: Option<Arc<std::sync::atomic::AtomicBool>>,
}

#[cfg(feature = "local")]
impl std::fmt::Debug for LocalEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalEngine").finish_non_exhaustive()
    }
}

#[cfg(feature = "local")]
impl Engine for LocalEngine {
    fn exec(&mut self, command: &str, params: &Value) -> Result<Value, EngineFailure> {
        let mut ws = self.ws.lock().map_err(|_| EngineFailure {
            code: "INTERNAL".into(),
            message: "workspace unavailable".into(),
        })?;
        ws.execute(&self.ctx, command, params).map_err(failure)
    }

    fn spawn(&mut self, command: &str, params: &Value) -> Option<Job> {
        let (tx, rx) = std::sync::mpsc::channel();
        let (ws, ctx) = (self.ws.clone(), self.ctx.clone());
        let (command, params) = (command.to_owned(), params.clone());
        std::thread::Builder::new()
            .name(format!("prswt {command}"))
            .spawn(move || {
                let r = match ws.lock() {
                    Ok(mut w) => w.execute(&ctx, &command, &params).map_err(failure),
                    Err(_) => Err(EngineFailure {
                        code: "INTERNAL".into(),
                        message: "workspace unavailable".into(),
                    }),
                };
                // The window may have closed meanwhile; nobody to tell then.
                let _ = tx.send(r);
            })
            .ok()?;
        Some(Job { rx })
    }

    fn cancel(&mut self) {
        if let Some(c) = &self.cancel {
            c.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// Engine error → the UI's backend-independent failure.
#[cfg(feature = "local")]
fn failure(e: prswt_core::EngineError) -> EngineFailure {
    EngineFailure {
        code: serde_json::to_value(e.code)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default(),
        message: e.message,
    }
}

/// Cached view data; cleared whenever the project, branch or state changes.
#[derive(Debug, Default)]
struct Cache {
    summary: Option<Value>,
    activities: Option<Value>,
    risks: Option<Value>,
    branches: Option<Value>,
    resources: Option<Value>,
    diff: Option<Value>,
    projects: Option<Value>,
    lookups: Option<Value>,
    /// `schedule.activity` of the inspected activity: (code, result).
    activity: Option<(String, Value)>,
    /// `schedule.activities` of the plan the Gantt is compared with.
    compare: Option<Value>,
}

/// The application state behind the window.
// Independent on/off UI switches (zoom fit, grouping, dialogs), not a state machine.
#[allow(clippy::struct_excessive_bools)]
pub struct PrswtApp {
    engine: Box<dyn Engine>,
    /// Ids of commands that change state (from `commands.list`).
    writes: Option<HashSet<String>>,
    /// Selected tab.
    pub tab: Tab,
    /// Selected project name (`None` until one is open).
    pub project: Option<String>,
    /// Selected branch name (`live` or a sandbox).
    pub branch: String,
    /// Last status or error line shown at the bottom.
    pub status: String,
    px_per_day: f32,
    /// Gantt zoom follows the window width until the user zooms.
    zoom_fit: bool,
    cache: Cache,
    /// Activity selected in the Activities view (its code).
    edit_activity: String,
    new_sandbox: String,
    iterations: u32,
    simulation: Option<Value>,
    /// File-menu actions waiting for the desktop shell.
    requests: Vec<FileRequest>,
    /// Paths offered under File ▸ Open recent (the shell keeps the list).
    pub recent: Vec<String>,
    new_project: Option<NewProject>,
    about: bool,
    /// Today's date, the default start of new projects. The shell sets it
    /// from the system clock (the UI reads no clock itself).
    pub today: jiff::civil::Date,
    running: Option<Running>,
    /// DSK-06: the open command dialog.
    form: Option<forms::OpenForm>,
    /// A report to show: (title, command result).
    report: Option<(String, Value)>,
    /// Title of the report a background command will produce.
    report_after: Option<String>,
    /// The Gantt chart groups activities under WBS summary rows.
    group_wbs: bool,
    /// The open command palette (Ctrl+K).
    palette: Option<chrome::Palette>,
    /// The selected project has changes not yet saved to its file. The
    /// shell sets it every frame (it knows the saved revisions).
    pub unsaved: bool,
    /// SYNC-01..04: account, cloud projects and sync state (the shell
    /// keeps it current).
    pub cloud: CloudView,
    cloud_requests: Vec<CloudRequest>,
    dialogs: cloud::CloudDialogs,
    /// Columns, sort and filters of the activity table.
    table: table::TableView,
    /// Draw baseline bars under the Gantt bars (when a baseline exists).
    show_baseline: bool,
    /// Gap 15: another plan drawn over the Gantt, `(project, branch)`.
    compare: Option<(String, String)>,
}

impl std::fmt::Debug for PrswtApp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrswtApp")
            .field("tab", &self.tab)
            .field("branch", &self.branch)
            .finish_non_exhaustive()
    }
}

impl PrswtApp {
    /// Desktop UI over a shared workspace (the control channel may share it too).
    #[cfg(feature = "local")]
    pub fn new(ws: Arc<Mutex<Workspace>>) -> Self {
        let cancel = ws.lock().ok().map(|w| w.cancel_handle());
        Self::with_engine(Box::new(LocalEngine {
            ws,
            ctx: Ctx::human("desktop"),
            cancel,
        }))
    }

    /// UI over any engine (tests pass a scripted one).
    pub fn with_engine(engine: Box<dyn Engine>) -> Self {
        Self {
            engine,
            writes: None,
            tab: Tab::Gantt,
            project: None,
            branch: "live".into(),
            status: "Create a project, open a .prswt file or a sample.".into(),
            px_per_day: 14.0,
            zoom_fit: true,
            cache: Cache::default(),
            edit_activity: String::new(),
            new_sandbox: "what-if".into(),
            iterations: 2000,
            simulation: None,
            requests: Vec::new(),
            recent: Vec::new(),
            new_project: None,
            about: false,
            today: jiff::civil::date(2027, 1, 4),
            running: None,
            form: None,
            report: None,
            report_after: None,
            group_wbs: true,
            palette: None,
            unsaved: false,
            cloud: CloudView::default(),
            cloud_requests: Vec::new(),
            dialogs: cloud::CloudDialogs::default(),
            table: table::TableView::default(),
            show_baseline: true,
            compare: None,
        }
    }

    /// File-menu actions queued since the last call (oldest first).
    pub fn take_requests(&mut self) -> Vec<FileRequest> {
        std::mem::take(&mut self.requests)
    }

    /// Queues a File-menu action.
    pub fn request(&mut self, r: FileRequest) {
        self.requests.push(r);
    }

    /// `(id, name)` of the selected project.
    pub fn selected(&mut self) -> Option<(String, String)> {
        let name = self.project.clone()?;
        Some((self.project_id(&name)?, name))
    }

    /// `(id, name)` of every open project.
    pub fn open_projects(&mut self) -> Vec<(String, String)> {
        self.projects().into_iter().map(|(n, id)| (id, n)).collect()
    }

    /// Change counter of an open project (`project.list` `revision`); the
    /// shell compares it with the value at the last save to show unsaved
    /// changes. Not cached, so agent edits over the control channel count.
    pub fn revision(&mut self, id: &str) -> Option<u64> {
        let v = self
            .engine
            .exec("project.list", &json!({ "brief": true }))
            .unwrap_or(Value::Null);
        v["projects"]
            .as_array()?
            .iter()
            .find(|p| p["project"] == id)?["revision"]
            .as_u64()
    }

    /// Text of the selected project from an exporting `command`
    /// (`file.save_project`, `file.export_xer`, …), or `None` with the
    /// reason on the status line.
    pub fn project_text(&mut self, command: &str) -> Option<String> {
        let params = if command == "file.save_project" {
            json!({})
        } else {
            self.with_branch(json!({}))
        };
        let r = self.exec(command, params)?;
        r["text"].as_str().map(str::to_owned)
    }

    /// Runs an importing `command` (`file.open_project`, `file.import_xer`,
    /// …) on `text`, selects the new project and returns its id.
    pub fn open_text(&mut self, command: &str, text: &str) -> Option<String> {
        let r = self.exec(command, json!({ "text": text, "project": null }))?;
        let id = r["project"].as_str()?.to_owned();
        self.select_id(&id);
        Some(id)
    }

    /// Closes an open project without asking (the shell asks first).
    pub fn close_project(&mut self, id: &str) -> bool {
        let ok = self
            .exec("project.close", json!({ "project": id }))
            .is_some();
        if ok {
            self.project = None;
            self.branch = "live".into();
            self.cache = Cache::default();
        }
        ok
    }

    /// Drops cached views so the next frame reads the engine again (after
    /// the shell changed a project, e.g. a cloud sync).
    pub fn refresh(&mut self) {
        self.cache = Cache::default();
    }

    /// Selects an activity by code, which opens the inspector on the
    /// Schedule and Activities views.
    pub fn select_activity(&mut self, code: &str) {
        code.clone_into(&mut self.edit_activity);
    }

    /// Selects the project with this id and its live branch.
    pub fn select_id(&mut self, id: &str) {
        self.cache = Cache::default();
        self.project = self.projects().into_iter().find(|p| p.1 == id).map(|p| p.0);
        self.branch = "live".into();
    }

    /// Runs a catalog command; errors go to the status line. Writes clear caches.
    pub fn exec(&mut self, command: &str, mut params: Value) -> Option<Value> {
        self.target(&mut params);
        let result = self.engine.exec(command, &params);
        self.finished(command, result)
    }

    /// Commands act on the selected project unless they name one.
    fn target(&self, params: &mut Value) {
        if let (Some(p), Some(m)) = (&self.project, params.as_object_mut())
            && !m.contains_key("project")
        {
            m.insert("project".into(), json!(p));
        }
    }

    /// DSK-05: starts a long command in the background (inline when the
    /// engine cannot). The window shows a busy panel until it finishes.
    pub fn exec_long(&mut self, command: &str, mut params: Value) {
        if self.running.is_some() {
            self.status = "Another command is still running.".into();
            return;
        }
        self.target(&mut params);
        match self.engine.spawn(command, &params) {
            Some(job) => {
                self.status = format!("{command}: running…");
                self.running = Some(Running {
                    command: command.to_owned(),
                    job,
                    started: None,
                });
            }
            None => {
                let r = self.engine.exec(command, &params);
                if let Some(v) = self.finished(command, r) {
                    self.after_long(command, &v);
                }
            }
        }
    }

    /// Whether a background command is running.
    pub fn busy(&self) -> bool {
        self.running.is_some()
    }

    /// Follow-up once a long command is done (load its results).
    fn after_long(&mut self, command: &str, v: &Value) {
        if let Some(title) = self.report_after.take() {
            self.report = Some((title, v.clone()));
        }
        if command == "simulation.run"
            && let Some(run) = v["run"].as_str()
        {
            self.simulation = self.exec(
                "simulation.get",
                json!({ "run": run, "include": ["histogram"] }),
            );
        }
    }

    /// Status line and cache handling for a finished command.
    fn finished(&mut self, command: &str, result: Result<Value, EngineFailure>) -> Option<Value> {
        match result {
            Ok(v) => {
                if self.is_write(command) {
                    self.cache = Cache::default();
                    self.status = format!("{command}: done");
                }
                Some(v)
            }
            Err(e) => {
                self.status = format!("{command}: {}", e.message);
                None
            }
        }
    }

    /// Opens a built-in sample and selects it.
    pub fn open_sample(&mut self, name: &str) {
        if let Some(id) = self
            .exec("sample.load", json!({ "name": name, "project": null }))
            .and_then(|r| r["project"].as_str().map(str::to_owned))
        {
            self.select_id(&id);
        }
    }

    /// Whether `command` changes state (unknown commands count as writes,
    /// which only costs a cache refresh).
    fn is_write(&mut self, command: &str) -> bool {
        if self.writes.is_none() {
            let list = self
                .engine
                .exec("commands.list", &json!({}))
                .unwrap_or(Value::Null);
            let ids: HashSet<String> = list["commands"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|c| c["writes"] == true)
                .filter_map(|c| c["id"].as_str().map(str::to_owned))
                .collect();
            if !ids.is_empty() {
                self.writes = Some(ids);
            }
        }
        self.writes.as_ref().is_none_or(|w| w.contains(command))
    }

    /// `(name, id)` of open projects, cached until the next write.
    fn projects(&mut self) -> Vec<(String, String)> {
        if self.cache.projects.is_none() {
            let v = self
                .engine
                .exec("project.list", &json!({ "brief": true }))
                .unwrap_or(Value::Null);
            self.cache.projects = Some(v);
        }
        self.cache.projects.as_ref().map_or_else(Vec::new, |v| {
            v["projects"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| {
                    Some((
                        p["name"].as_str()?.to_owned(),
                        p["project"].as_str()?.to_owned(),
                    ))
                })
                .collect()
        })
    }

    fn project_names(&mut self) -> Vec<String> {
        self.projects().into_iter().map(|p| p.0).collect()
    }

    fn project_id(&mut self, name: &str) -> Option<String> {
        self.projects()
            .into_iter()
            .find(|p| p.0 == name)
            .map(|p| p.1)
    }

    fn has_project(&mut self) -> bool {
        !self.projects().is_empty()
    }

    fn with_branch(&self, mut v: Value) -> Value {
        if let Some(m) = v.as_object_mut() {
            m.insert("branch".into(), json!(self.branch));
        }
        v
    }

    fn cached(
        &mut self,
        slot: fn(&mut Cache) -> &mut Option<Value>,
        command: &str,
        params: Value,
    ) -> Value {
        if let Some(v) = slot(&mut self.cache) {
            return v.clone();
        }
        let params = self.with_branch(params);
        let v = self.exec(command, params).unwrap_or(Value::Null);
        *slot(&mut self.cache) = Some(v.clone());
        v
    }

    /// Draws the whole window into `ui`: menu bar, navigation, header,
    /// status bar, inspector and the selected view (design B).
    pub fn ui(&mut self, ui: &mut Ui) {
        if self.background(ui) {
            return;
        }
        if self.project.is_none() {
            self.project = self.project_names().first().cloned();
        }
        self.shortcuts(ui);
        let tok = Tokens::for_ui(ui);
        egui::Panel::top("menu")
            .frame(
                egui::Frame::new()
                    .fill(tok.sidebar)
                    .inner_margin(egui::Margin::symmetric(8, 2)),
            )
            .show(ui, |ui| self.menu_bar(ui));
        self.sidebar(ui);
        let open = self.has_project();
        if open {
            self.header(ui);
        }
        self.dialogs(ui);
        self.cloud_dialogs(ui);
        self.palette_ui(ui);
        self.status_bar(ui);
        if open {
            self.inspector(ui);
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(tok.bg)
                    .inner_margin(egui::Margin::symmetric(20, 16)),
            )
            .show(ui, |ui| {
                if !open {
                    self.welcome(ui);
                    return;
                }
                self.kpis(ui);
                ui.add_space(12.0);
                match self.tab {
                    Tab::Gantt => self.gantt(ui),
                    tab => {
                        chrome::card(&tok).show(ui, |ui| {
                            ui.set_min_size(ui.available_size());
                            match tab {
                                Tab::Activities => self.activities(ui),
                                Tab::Network => {
                                    let rows = self.rows();
                                    let sel = self.edit_activity.clone();
                                    if let Some(code) = network::draw(ui, &tok, &rows, &sel) {
                                        self.edit_activity =
                                            if code == sel { String::new() } else { code };
                                    }
                                }
                                other => {
                                    egui::ScrollArea::vertical()
                                        .auto_shrink([false, false])
                                        .show(ui, |ui| match other {
                                            Tab::Risks => self.risks(ui),
                                            Tab::Simulation => self.simulation_view(ui),
                                            Tab::Sandboxes => self.sandboxes(ui),
                                            _ => self.resources(ui),
                                        });
                                }
                            }
                        });
                    }
                }
            });
    }

    /// DSK-05: while a command runs in the background the workspace is
    /// locked, so the frame shows only a busy panel (no engine calls) and
    /// keeps repainting. Returns whether it drew that panel.
    fn background(&mut self, ui: &mut Ui) -> bool {
        let now = ui.input(|i| i.time);
        let Some(run) = &mut self.running else {
            return false;
        };
        if let Some(result) = run.job.poll() {
            let command = run.command.clone();
            self.running = None;
            if let Some(v) = self.finished(&command, result) {
                self.status = format!("{command}: done");
                self.after_long(&command, &v);
            }
            return false;
        }
        let secs = format!("{:.0}", (now - *run.started.get_or_insert(now)).max(0.0));
        let what = match run.command.as_str() {
            "simulation.run" => "Running risk analysis",
            "resource.level" => "Levelling resources",
            other => other,
        };
        let mut stop = false;
        egui::CentralPanel::default().show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.4);
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.heading(format!("{what}… {secs} s"));
                });
                ui.add_space(8.0);
                stop = ui
                    .button("Cancel")
                    .on_hover_text("Stop and keep the project as it was")
                    .clicked()
                    || ui.input(|i| i.key_pressed(egui::Key::Escape));
            });
        });
        if stop {
            self.engine.cancel();
            self.status = format!("{what}: stopping…");
        }
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
        true
    }

    /// DSK-04: Ctrl (⌘ on macOS) shortcuts. Shifted variants are consumed
    /// first because egui also matches Ctrl+S on Ctrl+Shift+S.
    fn shortcuts(&mut self, ui: &mut Ui) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let cmd = |k| KeyboardShortcut::new(Modifiers::COMMAND, k);
        let shift = |k| KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, k);
        let (new, open, save_as, save, redo_z, undo, redo, close) = ui.input_mut(|i| {
            (
                i.consume_shortcut(&cmd(Key::N)),
                i.consume_shortcut(&cmd(Key::O)),
                i.consume_shortcut(&shift(Key::S)),
                i.consume_shortcut(&cmd(Key::S)),
                i.consume_shortcut(&shift(Key::Z)),
                i.consume_shortcut(&cmd(Key::Z)),
                i.consume_shortcut(&cmd(Key::Y)),
                i.consume_shortcut(&cmd(Key::W)),
            )
        });
        let (palette, refresh) = ui.input_mut(|i| {
            (
                i.consume_shortcut(&cmd(Key::K)) || i.consume_shortcut(&cmd(Key::P)),
                i.key_pressed(Key::F9),
            )
        });
        if palette {
            self.palette = Some(chrome::Palette::default());
        }
        if refresh {
            self.cache = Cache::default();
        }
        if self.project.is_some() {
            for t in Tab::ALL {
                if ui.input_mut(|i| i.consume_shortcut(&cmd(t.key()))) {
                    self.tab = t;
                }
            }
        }
        if new {
            self.new_project = Some(NewProject::starting(self.today));
        }
        for (hit, r) in [
            (open, FileRequest::Open),
            (save_as, FileRequest::SaveAs),
            (save, FileRequest::Save),
            (close, FileRequest::Close),
        ] {
            if hit {
                self.requests.push(r);
            }
        }
        if undo {
            self.edit("edit.undo");
        }
        if redo || redo_z {
            self.edit("edit.redo");
        }
    }

    /// Undo / redo on the selected branch.
    fn edit(&mut self, command: &str) {
        if self.project.is_some() {
            let p = self.with_branch(json!({}));
            self.exec(command, p);
        }
    }

    /// DSK-04: menus named after P6's (File, Edit, View, Project, Help).
    fn menu_bar(&mut self, ui: &mut Ui) {
        use egui::{Button, Key, KeyboardShortcut, Modifiers};
        let keys = |ui: &Ui, k| {
            ui.ctx()
                .format_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, k))
        };
        let open = self.project.is_some();
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui
                    .add(Button::new("New project…").shortcut_text(keys(ui, Key::N)))
                    .clicked()
                {
                    self.new_project = Some(NewProject::starting(self.today));
                }
                if ui
                    .add(Button::new("Open…").shortcut_text(keys(ui, Key::O)))
                    .clicked()
                {
                    self.requests.push(FileRequest::Open);
                }
                ui.menu_button("Open recent", |ui| {
                    if self.recent.is_empty() {
                        ui.label("No recent files");
                    }
                    for path in self.recent.clone() {
                        if ui.button(&path).clicked() {
                            self.requests.push(FileRequest::OpenRecent(path));
                        }
                    }
                });
                ui.menu_button("Open sample", |ui| {
                    for s in SAMPLES {
                        if ui.button(s).clicked() {
                            self.open_sample(s);
                        }
                    }
                });
                self.cloud_menu(ui);
                ui.separator();
                if ui
                    .add_enabled(open, Button::new("Save").shortcut_text(keys(ui, Key::S)))
                    .clicked()
                {
                    self.requests.push(FileRequest::Save);
                }
                if ui.add_enabled(open, Button::new("Save as…")).clicked() {
                    self.requests.push(FileRequest::SaveAs);
                }
                ui.separator();
                ui.menu_button("Import", |ui| {
                    for f in Format::ALL {
                        if ui.button(format!("{}…", f.label())).clicked() {
                            self.requests.push(FileRequest::Import(f));
                        }
                    }
                });
                ui.add_enabled_ui(open, |ui| {
                    ui.menu_button("Export", |ui| {
                        for f in Format::ALL {
                            if ui.button(format!("{}…", f.label())).clicked() {
                                self.requests.push(FileRequest::Export(f));
                            }
                        }
                    });
                });
                ui.separator();
                if ui
                    .add_enabled(
                        open,
                        Button::new("Close project").shortcut_text(keys(ui, Key::W)),
                    )
                    .clicked()
                {
                    self.requests.push(FileRequest::Close);
                }
                if ui.button("Quit").clicked() {
                    self.requests.push(FileRequest::Quit);
                }
            });
            ui.menu_button("Edit", |ui| {
                if ui
                    .add_enabled(open, Button::new("Undo").shortcut_text(keys(ui, Key::Z)))
                    .clicked()
                {
                    self.edit("edit.undo");
                }
                if ui
                    .add_enabled(open, Button::new("Redo").shortcut_text(keys(ui, Key::Y)))
                    .clicked()
                {
                    self.edit("edit.redo");
                }
            });
            ui.menu_button("View", |ui| {
                for t in Tab::ALL {
                    if ui.selectable_label(self.tab == t, t.label()).clicked() {
                        self.tab = t;
                    }
                }
                ui.separator();
                ui.menu_button("Theme", |ui| {
                    let mut pref = ui.ctx().options(|o| o.theme_preference);
                    ui.radio_value(&mut pref, egui::ThemePreference::System, "Follow system");
                    ui.radio_value(&mut pref, egui::ThemePreference::Light, "Light");
                    ui.radio_value(&mut pref, egui::ThemePreference::Dark, "Dark");
                    ui.ctx().set_theme(pref);
                });
            });
            ui.menu_button("Project", |ui| {
                if ui
                    .add_enabled(open, Button::new("Schedule").shortcut_text("F9"))
                    .clicked()
                {
                    self.cache = Cache::default();
                }
                if ui
                    .add_enabled(open, Button::new("What-if branches"))
                    .clicked()
                {
                    self.tab = Tab::Sandboxes;
                }
                ui.separator();
                self.form_items(ui, "Project", open);
            });
            for menu in ["Enterprise", "Risks", "Tools", "Reports"] {
                ui.menu_button(menu, |ui| self.form_items(ui, menu, open));
            }
            ui.menu_button("Help", |ui| {
                self.form_items(ui, "Help", true);
                if ui.button("About PRSWT").clicked() {
                    self.about = true;
                }
            });
        });
    }

    /// Menu items of every command dialog that belongs in `menu`.
    fn form_items(&mut self, ui: &mut Ui, menu: &str, enabled: bool) {
        for spec in forms::FORMS.iter().filter(|f| f.menu == menu) {
            let needs_project = spec.command != "commands.list";
            if ui
                .add_enabled(enabled || !needs_project, egui::Button::new(spec.title))
                .clicked()
            {
                self.open_form(spec);
            }
        }
    }

    /// Opens a command dialog; a dialog without fields runs at once.
    fn open_form(&mut self, spec: &'static forms::FormSpec) {
        let lookups = self.lookups();
        let form = forms::OpenForm::new(spec, self.today, &lookups);
        if spec.fields.is_empty() {
            self.run_form(&form);
        } else {
            self.form = Some(form);
        }
    }

    /// Pick lists for the selected project and branch (cached).
    fn lookups(&mut self) -> Value {
        if self.project.is_none() {
            return Value::Null;
        }
        self.cached(|c| &mut c.lookups, "project.lookups", json!({}))
    }

    /// Runs a dialog's command; returns whether it succeeded.
    fn run_form(&mut self, form: &forms::OpenForm) -> bool {
        let spec = form.spec;
        let params = if spec.command == "commands.list" {
            form.params()
        } else {
            self.with_branch(form.params())
        };
        let title = spec.title.trim_end_matches('…').to_owned();
        if LONG_COMMANDS.contains(&spec.command) {
            if spec.shows_result {
                self.report_after = Some(title);
            }
            self.exec_long(spec.command, params);
            return true;
        }
        match self.exec(spec.command, params) {
            Some(v) => {
                if spec.shows_result {
                    self.report = Some((title, v));
                }
                true
            }
            None => false,
        }
    }

    /// The open command dialog and the report window.
    fn command_dialogs(&mut self, ui: &mut Ui) {
        if let Some(mut form) = self.form.take() {
            let lookups = self.lookups();
            let status = self.status.clone();
            let (mut run, mut cancel) = (false, false);
            let r = egui::Modal::new(egui::Id::unique("command_form")).show(ui.ctx(), |ui| {
                ui.heading(form.spec.title.trim_end_matches('…'));
                ui.add_space(6.0);
                form.ui(ui, &lookups);
                if status.starts_with(form.spec.command) && !status.ends_with("done") {
                    ui.colored_label(ui.visuals().error_fg_color, &status);
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    run = ui
                        .add_enabled(form.complete(), egui::Button::new("OK"))
                        .clicked();
                    cancel = ui.button("Cancel").clicked();
                });
            });
            let keep = !(cancel || r.should_close() || (run && self.run_form(&form)));
            if keep {
                self.form = Some(form);
            }
        }
        if let Some((title, v)) = self.report.take() {
            let r = egui::Modal::new(egui::Id::unique("report")).show(ui.ctx(), |ui| {
                ui.set_min_width(520.0);
                ui.heading(&title);
                forms::result_tree(ui, &v);
                ui.button("Close").clicked()
            });
            if !(r.inner || r.should_close()) {
                self.report = Some((title, v));
            }
        }
    }

    /// File ▸ New project and Help ▸ About.
    fn dialogs(&mut self, ui: &mut Ui) {
        self.command_dialogs(ui);
        if let Some(mut np) = self.new_project.take() {
            let (mut create, mut cancel) = (false, false);
            let status = self.status.clone();
            let r = egui::Modal::new(egui::Id::unique("new_project")).show(ui.ctx(), |ui| {
                ui.heading("New project");
                egui::Grid::new("np").num_columns(2).show(ui, |ui| {
                    ui.label("Name");
                    ui.text_edit_singleline(&mut np.name);
                    ui.end_row();
                    ui.label("Start date");
                    ui.add(
                        egui_extras::DatePickerButton::new(&mut np.date)
                            .id_salt("np_date")
                            .calendar_week(true)
                            .highlight_weekends(true),
                    );
                    ui.end_row();
                    ui.label("Start time");
                    egui::ComboBox::from_id_salt("np_hour")
                        .selected_text(format!("{:02}:00", np.hour))
                        .show_ui(ui, |ui| {
                            for h in 0..24 {
                                ui.selectable_value(&mut np.hour, h, format!("{h:02}:00"));
                            }
                        });
                    ui.end_row();
                    ui.label("Work calendar");
                    let label = |id: &str| {
                        CALENDAR_PRESETS
                            .iter()
                            .find(|c| c.0 == id)
                            .map_or(id.to_owned(), |c| c.1.to_owned())
                    };
                    egui::ComboBox::from_id_salt("np_cal")
                        .selected_text(label(np.calendar))
                        .show_ui(ui, |ui| {
                            for (id, text) in CALENDAR_PRESETS {
                                ui.selectable_value(&mut np.calendar, id, text);
                            }
                        });
                    ui.end_row();
                });
                if status.starts_with("project.create") {
                    ui.colored_label(ui.visuals().error_fg_color, &status);
                }
                ui.horizontal(|ui| {
                    let ok = !np.name.trim().is_empty();
                    create = ui.add_enabled(ok, egui::Button::new("Create")).clicked()
                        || (ok && ui.input(|i| i.key_pressed(egui::Key::Enter)));
                    cancel = ui.button("Cancel").clicked();
                });
            });
            let mut keep = !(cancel || r.should_close());
            if create {
                let created = self
                    .exec(
                        "project.create",
                        json!({ "name": np.name.trim(), "start": np.start(), "calendar": np.calendar, "project": null }),
                    )
                    .and_then(|r| r["project"].as_str().map(str::to_owned));
                if let Some(id) = created {
                    self.select_id(&id);
                    keep = false;
                }
            }
            if keep {
                self.new_project = Some(np);
            }
        }
        if self.about {
            let r = egui::Modal::new(egui::Id::unique("about")).show(ui.ctx(), |ui| {
                ui.heading("PRSWT");
                ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                ui.label("Project controls and risk: CPM scheduling, resources, EVM, Monte Carlo.");
                ui.button("Close").clicked()
            });
            if r.inner || r.should_close() {
                self.about = false;
            }
        }
    }

    fn rows(&mut self) -> Vec<Value> {
        if self.cache.activities.is_none() {
            let params = self.with_branch(json!({ "links": true }));
            let v = self.all_activities(&params).unwrap_or(Value::Null);
            self.cache.activities = Some(v);
        }
        self.cache
            .activities
            .as_ref()
            .and_then(|v| v["items"].as_array().cloned())
            .unwrap_or_default()
    }

    /// Every `schedule.activities` row, page by page (the command returns
    /// at most 1000 per call), up to [`MAX_ROWS`]: `{ total, items }`.
    fn all_activities(&mut self, params: &Value) -> Option<Value> {
        let mut items: Vec<Value> = Vec::new();
        loop {
            let mut p = params.clone();
            p["limit"] = json!(ROW_PAGE);
            p["offset"] = json!(items.len());
            let v = self.exec("schedule.activities", p)?;
            let page = v["items"].as_array().cloned().unwrap_or_default();
            let total = v["total"]
                .as_u64()
                .and_then(|t| usize::try_from(t).ok())
                .unwrap_or(0);
            let empty = page.is_empty();
            items.extend(page);
            if empty || items.len() >= total.min(MAX_ROWS) {
                return Some(json!({ "total": total, "items": items }));
            }
        }
    }

    /// Rows of the plan being compared with (cached like the others).
    fn compare_rows(&mut self) -> Vec<Value> {
        let Some((project, branch)) = self.compare.clone() else {
            return Vec::new();
        };
        if self.cache.compare.is_none() {
            let v = self
                .all_activities(&json!({ "project": project, "branch": branch }))
                .unwrap_or(Value::Null);
            if v.is_null() {
                // The branch or project went away: stop comparing.
                self.compare = None;
            }
            self.cache.compare = Some(v);
        }
        self.cache
            .compare
            .as_ref()
            .and_then(|v| v["items"].as_array().cloned())
            .unwrap_or_default()
    }

    /// The "Compare with" menu: other branches of this project, other projects.
    fn compare_menu(&mut self, ui: &mut Ui) {
        let label = match &self.compare {
            Some((p, b)) if Some(p) == self.project.as_ref() => format!("Compare: {b}"),
            Some((p, _)) => format!("Compare: {p}"),
            None => "Compare…".to_owned(),
        };
        ui.menu_button(label, |ui| {
            if ui.selectable_label(self.compare.is_none(), "Off").clicked() {
                self.compare = None;
                self.cache.compare = None;
            }
            let branches = self.cached(|c| &mut c.branches, "sandbox.list", json!({}));
            let here = self.project.clone().unwrap_or_default();
            let mut choices: Vec<(String, String, String)> = branches["branches"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|b| b["status"] == "open")
                .filter_map(|b| b["name"].as_str().map(str::to_owned))
                .filter(|n| *n != self.branch)
                .map(|n| (format!("Branch {n}"), here.clone(), n))
                .collect();
            choices.extend(
                self.project_names()
                    .into_iter()
                    .filter(|p| *p != here)
                    .map(|p| (format!("Project {p}"), p, "live".to_owned())),
            );
            if choices.is_empty() {
                ui.label(
                    RichText::new("Make a what-if branch or open another project first").weak(),
                );
            }
            for (label, project, branch) in choices {
                let on = self.compare.as_ref() == Some(&(project.clone(), branch.clone()));
                if ui.selectable_label(on, label).clicked() {
                    self.compare = Some((project, branch));
                    self.cache.compare = None;
                }
            }
        });
    }

    /// UI-02 / DSK-07: Gantt chart in a card: a toolbar (Gantt/Table,
    /// WBS grouping, legend, zoom), the activity table on the left (code
    /// and name, duration, start, finish, total float) and bars on a dated
    /// timeline. WBS summary rows carry thin roll-up bars; the longest path
    /// is orange; clicking a row opens the inspector.
    fn gantt(&mut self, ui: &mut Ui) {
        let rows = self.rows();
        let other: std::collections::HashMap<String, (Option<i64>, Option<i64>)> = self
            .compare_rows()
            .iter()
            .filter_map(|r| {
                Some((
                    r["code"].as_str()?.to_owned(),
                    (minutes(&r["early_start"]), minutes(&r["early_finish"])),
                ))
            })
            .collect();
        let compare_name = self.compare.as_ref().map(|(p, b)| {
            if Some(p) == self.project.as_ref() {
                b.clone()
            } else {
                p.clone()
            }
        });
        let summary = self.cached(|c| &mut c.summary, "schedule.run", json!({}));
        let lookups = self.lookups();
        let tok = Tokens::for_ui(ui);
        chrome::card(&tok).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ui.horizontal(|ui| {
                if chrome::segmented(ui, &tok, &["Gantt", "Table"], 0) == Some(1) {
                    self.tab = Tab::Activities;
                }
                ui.checkbox(&mut self.group_wbs, "Group by WBS");
                if rows.iter().any(|r| r["baseline_finish"].is_string()) {
                    ui.checkbox(&mut self.show_baseline, "Baseline")
                        .on_hover_text("Grey bars: dates of the latest baseline");
                }
                self.compare_menu(ui);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Right to left: the list reads Fit, Month, Week, Day on screen.
                    let labels: Vec<&str> = ZOOMS.iter().map(|z| z.0).chain(["Fit"]).collect();
                    let current = if self.zoom_fit {
                        ZOOMS.len()
                    } else {
                        ZOOMS
                            .iter()
                            .position(|z| (z.1 - self.px_per_day).abs() < 0.01)
                            .unwrap_or(usize::MAX)
                    };
                    match chrome::segmented(ui, &tok, &labels, current) {
                        Some(k) if k == ZOOMS.len() => self.zoom_fit = true,
                        Some(k) => {
                            if let Some(z) = ZOOMS.get(k) {
                                self.px_per_day = z.1;
                                self.zoom_fit = false;
                            }
                        }
                        None => {}
                    }
                    ui.add_space(8.0);
                    legend(ui, &tok);
                });
            });
            ui.add_space(6.0);
            let mut lines = gantt_lines(
                &rows,
                &lookups,
                self.project.as_deref().unwrap_or(""),
                self.group_wbs,
            );
            for l in &mut lines {
                if let Some(&(s, f)) = other.get(&l.code) {
                    (l.cs, l.cf) = (s, f);
                }
            }
            if let Some(name) = &compare_name {
                let mine = lines.iter().filter_map(|l| l.ef).max();
                let theirs = other.values().filter_map(|d| d.1).max();
                if let (Some(a), Some(b)) = (mine, theirs) {
                    let days = (b - a) as f64 / f64::from(DAY_MINUTES);
                    ui.label(
                        RichText::new(format!(
                            "Outlined bars: {name}. Its finish is {} ({:+.1} calendar days).",
                            long_date(&iso(b)),
                            days
                        ))
                        .color(tok.muted),
                    );
                }
            }
            let starts = lines.iter().filter_map(|l| l.es);
            let ends = lines.iter().filter_map(|l| l.lf.max(l.ef));
            let early_end = lines.iter().filter_map(|l| l.ef).max();
            let (Some(t0), Some(t1), Some(fit_end)) = (starts.min(), ends.max(), early_end) else {
                Self::empty_state(
                    ui,
                    "No activities yet",
                    "Add the first one with + Activity in the header.",
                );
                return;
            };
            // Start the axis on the Monday before the first bar.
            // 1970-01-01 was a Thursday, so day d is a Monday when (d + 3) % 7 == 0.
            let day = t0.div_euclid(1440);
            let t0 = (day - (day + 3).rem_euclid(7)) * 1440;
            let span_days = ((t1 - t0).max(1440) as f32 / DAY_MINUTES).ceil() + 7.0;
            let table_w: f32 = GANTT_COLS.iter().map(|c| c.1).sum();
            // Fit shows the bars; float lines past the last finish scroll.
            if self.zoom_fit {
                let fit_days = ((fit_end - t0).max(1440) as f32 / DAY_MINUTES).ceil() + 10.0;
                let avail = (ui.available_width() - table_w - 24.0).max(200.0);
                self.px_per_day = (avail / fit_days).clamp(1.0, 60.0);
            }
            let ppd = self.px_per_day;
            let show_baseline = self.show_baseline;
            let header = 40.0;
            let width = table_w + span_days * ppd + 16.0;
            let height = header + ROW_H * lines.len() as f32 + 8.0;
            let data_date = minutes(&summary["data_date"]);
            let selected = self.edit_activity.clone();
            let mut clicked: Option<String> = None;
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let (resp, painter) =
                        ui.allocate_painter(Vec2::new(width, height), Sense::click());
                    let o = resp.rect.min;
                    let font = FontId::proportional(13.0);
                    let small = FontId::proportional(11.0);
                    let chart_x = o.x + table_w;
                    let x = |t: i64| chart_x + (t - t0) as f32 / DAY_MINUTES * ppd;
                    let hover = resp.hover_pos();
                    // Row highlights first, so grid lines and bars draw on top.
                    for (k, l) in lines.iter().enumerate() {
                        let y = o.y + header + ROW_H * k as f32;
                        let row = Rect::from_min_size(Pos2::new(o.x, y), Vec2::new(width, ROW_H));
                        if !l.summary && l.code == selected {
                            painter.rect_filled(row, 0.0, tok.accent_soft);
                        } else if hover.is_some_and(|p| row.contains(p)) {
                            painter.rect_filled(row, 0.0, ui.visuals().faint_bg_color);
                        }
                    }
                    painter.line_segment(
                        [
                            Pos2::new(o.x, o.y + header),
                            Pos2::new(o.x + width, o.y + header),
                        ],
                        Stroke::new(1.0, tok.line),
                    );
                    let mut cx = o.x;
                    for (title, w, align) in GANTT_COLS {
                        let pos = if align == Align2::RIGHT_CENTER {
                            Pos2::new(cx + w - 8.0, o.y + header / 2.0)
                        } else {
                            Pos2::new(cx + 12.0, o.y + header / 2.0)
                        };
                        painter.text(pos, align, title, semibold(10.5), tok.muted);
                        cx += w;
                    }
                    timeline_header(
                        &painter, chart_x, o.y, header, t0, span_days, ppd, &small, tok.ink,
                        tok.muted, tok.grid, height,
                    );
                    for (k, l) in lines.iter().enumerate() {
                        let y = o.y + header + ROW_H * k as f32;
                        let mid = y + ROW_H / 2.0;
                        let row = Rect::from_min_size(Pos2::new(o.x, y), Vec2::new(width, ROW_H));
                        // Activity cell: indent, small code, then the name.
                        let mut nx = o.x + 12.0 + f32::from(l.level) * 14.0;
                        if !l.code.is_empty() {
                            let g =
                                painter.layout_no_wrap(l.code.clone(), small.clone(), tok.muted);
                            let gw = g.size().x;
                            painter.galley(Pos2::new(nx, mid - g.size().y / 2.0), g, tok.muted);
                            nx += gw + 6.0;
                        }
                        let name_w = (o.x + GANTT_COLS[0].1 - nx - 8.0).max(10.0);
                        let name_font = if l.summary {
                            semibold(13.0)
                        } else {
                            font.clone()
                        };
                        painter.text(
                            Pos2::new(nx, mid),
                            Align2::LEFT_CENTER,
                            clip(&l.name, name_w, 6.9),
                            name_font,
                            tok.ink,
                        );
                        let cells = [
                            l.dur.clone(),
                            short_date(&l.start),
                            short_date(&l.finish),
                            l.float.clone(),
                        ];
                        let mut cx = o.x + GANTT_COLS[0].1;
                        for ((_, w, _), cell) in GANTT_COLS.iter().skip(1).zip(cells) {
                            painter.text(
                                Pos2::new(cx + w - 8.0, mid),
                                Align2::RIGHT_CENTER,
                                cell,
                                font.clone(),
                                if l.summary { tok.ink } else { tok.muted },
                            );
                            cx += w;
                        }
                        let (Some(es), Some(ef)) = (l.es, l.ef) else {
                            continue;
                        };
                        let colour = if l.on_path { tok.critical } else { tok.normal };
                        let baseline = show_baseline && !l.summary && l.bs.is_some();
                        if !l.summary
                            && let (Some(cs), Some(cf)) = (l.cs, l.cf)
                        {
                            // The compared plan: an outline around where its bar would be.
                            let bar = Rect::from_min_max(
                                Pos2::new(x(cs), mid - 8.0),
                                Pos2::new(x(cf).max(x(cs) + 3.0), mid + 6.0),
                            );
                            painter.rect_stroke(
                                bar,
                                6.0_f32.min(bar.width() / 2.0),
                                Stroke::new(1.5, tok.accent),
                                egui::StrokeKind::Outside,
                            );
                        }
                        if baseline && let (Some(bs), Some(bf)) = (l.bs, l.bf) {
                            let bar = Rect::from_min_max(
                                Pos2::new(x(bs), mid + 7.0),
                                Pos2::new(x(bf).max(x(bs) + 2.0), mid + 10.0),
                            );
                            painter.rect_filled(bar, 1.0, tok.summary.gamma_multiply(0.6));
                        }
                        if !l.summary
                            && let Some(lf) = l.lf
                            && lf > ef
                        {
                            // Below the bar (and the baseline bar), clear of the label.
                            let s = Stroke::new(1.0, tok.float);
                            let fy = if baseline { mid + 12.5 } else { mid + 8.0 };
                            painter.line_segment([Pos2::new(x(ef), fy), Pos2::new(x(lf), fy)], s);
                            painter.line_segment(
                                [Pos2::new(x(lf), fy - 3.0), Pos2::new(x(lf), fy + 3.0)],
                                s,
                            );
                        }
                        let label_x = if l.summary {
                            let bar = Rect::from_min_max(
                                Pos2::new(x(es), mid - 2.0),
                                Pos2::new(x(ef).max(x(es) + 2.0), mid + 2.0),
                            );
                            painter.rect_filled(bar, 2.0, tok.summary);
                            None
                        } else if l.milestone {
                            let c = Pos2::new(x(es), mid);
                            let d = 6.0;
                            painter.add(egui::Shape::convex_polygon(
                                vec![
                                    c + Vec2::new(0.0, -d),
                                    c + Vec2::new(d, 0.0),
                                    c + Vec2::new(0.0, d),
                                    c + Vec2::new(-d, 0.0),
                                ],
                                if l.on_path {
                                    tok.critical
                                } else {
                                    tok.milestone
                                },
                                Stroke::NONE,
                            ));
                            Some(c.x + 10.0)
                        } else {
                            let bar = Rect::from_min_max(
                                Pos2::new(x(es), mid - 6.0),
                                Pos2::new(x(ef).max(x(es) + 3.0), mid + 6.0),
                            );
                            painter.rect_filled(bar, 6.0_f32.min(bar.width() / 2.0), colour);
                            Some(bar.max.x + 6.0)
                        };
                        // Names beside the bars when there is room for them.
                        if let Some(lx) = label_x
                            && ppd >= 3.0
                        {
                            painter.text(
                                Pos2::new(lx, mid),
                                Align2::LEFT_CENTER,
                                clip(&l.name, 160.0, 6.2),
                                small.clone(),
                                tok.muted,
                            );
                        }
                        if let Some(pos) = hover
                            && row.contains(pos)
                        {
                            let tip = format!(
                                "{}  {}\nStart {}   Finish {}\nLate finish {}   Total float {}{}{}",
                                l.code,
                                l.name,
                                long_date(&l.start),
                                long_date(&l.finish),
                                long_date(&l.late_finish),
                                l.float,
                                match (l.bs.is_some(), l.cs.zip(l.cf)) {
                                    (_, Some((cs, cf))) => format!(
                                        "\n{}: {} – {}   Finish shift {:+.1} d",
                                        compare_name.as_deref().unwrap_or("Compared"),
                                        day_date(&iso(cs)),
                                        day_date(&iso(cf)),
                                        (l.ef.unwrap_or(cf) - cf) as f64 / f64::from(DAY_MINUTES)
                                    ),
                                    (true, None) => format!(
                                        "\nBaseline {} – {}   Variance {}",
                                        short_date(&l.baseline_start),
                                        short_date(&l.baseline_finish),
                                        l.variance
                                    ),
                                    (false, None) => String::new(),
                                },
                                if l.on_path && !l.summary {
                                    "\nOn the longest path"
                                } else {
                                    ""
                                },
                            );
                            resp.clone().on_hover_text_at_pointer(tip);
                            if resp.clicked() && !l.summary {
                                clicked = Some(l.code.clone());
                            }
                        }
                    }
                    if let Some(dd) = data_date {
                        let dx = x(dd);
                        painter.line_segment(
                            [Pos2::new(dx, o.y + header), Pos2::new(dx, o.y + height)],
                            Stroke::new(2.0, tok.data_date),
                        );
                    }
                    // Separator between table and chart.
                    painter.line_segment(
                        [Pos2::new(chart_x, o.y), Pos2::new(chart_x, o.y + height)],
                        Stroke::new(1.0, tok.line),
                    );
                });
            if let Some(code) = clicked {
                self.edit_activity = if code == selected {
                    String::new()
                } else {
                    code
                };
            }
        });
    }

    /// Start screen when no project is open: the three ways in.
    fn welcome(&mut self, ui: &mut Ui) {
        let tok = Tokens::for_ui(ui);
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() * 0.18).max(12.0));
            ui.label(
                RichText::new("Plan, schedule and stress-test a project")
                    .font(semibold(24.0))
                    .color(tok.ink),
            );
            ui.label(
                RichText::new(
                    "CPM scheduling, resources, earned value and Monte Carlo risk, on your computer. No account needed.",
                )
                .color(tok.muted),
            );
            ui.add_space(22.0);
            let size = Vec2::new(300.0, 40.0);
            if ui
                .add(
                    egui::Button::new(
                        RichText::new("New project…")
                            .font(semibold(14.0))
                            .color(Color32::WHITE),
                    )
                    .fill(tok.accent)
                    .stroke(Stroke::NONE)
                    .corner_radius(8)
                    .min_size(size),
                )
                .clicked()
            {
                self.new_project = Some(NewProject::starting(self.today));
            }
            ui.add_space(6.0);
            if ui
                .add(egui::Button::new("Open project file…").min_size(size))
                .clicked()
            {
                self.requests.push(FileRequest::Open);
            }
            ui.add_space(10.0);
            ui.menu_button(RichText::new("Open a sample project").color(tok.accent), |ui| {
                for s in SAMPLES {
                    if ui.button(s).clicked() {
                        self.open_sample(s);
                    }
                }
            });
            if !self.recent.is_empty() {
                ui.add_space(22.0);
                chrome::card(&tok).show(ui, |ui| {
                    ui.set_width(460.0);
                    ui.label(RichText::new("RECENT").font(semibold(11.0)).color(tok.muted));
                    for path in self.recent.clone().into_iter().take(6) {
                        if ui
                            .link(clip(&path, 440.0, 6.8))
                            .on_hover_text(&path)
                            .clicked()
                        {
                            self.requests.push(FileRequest::OpenRecent(path));
                        }
                    }
                });
            }
            ui.add_space(18.0);
            ui.label(
                RichText::new("You can also drop a .prswt, .xer or .xml file on this window.")
                    .small()
                    .color(tok.muted),
            );
        });
    }

    /// A centred placeholder for views with nothing to show yet.
    fn empty_state(ui: &mut Ui, title: &str, hint: &str) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.3);
            ui.heading(title);
            ui.label(RichText::new(hint).weak());
        });
    }

    /// UI-03 / DSK-06 / gap 14: activity table. Rows are selected by
    /// clicking; the buttons open the command dialogs with the selection
    /// filled in. Headers sort (click again to reverse); the second toolbar
    /// filters, picks columns, exports CSV and prints.
    fn activities(&mut self, ui: &mut Ui) {
        let rows = self.rows();
        let selected = self.edit_activity.clone();
        ui.horizontal(|ui| {
            if ui.button("+ Add activity…").clicked() {
                self.open_command("activity.add", &[]);
            }
            ui.add_enabled_ui(!selected.is_empty(), |ui| {
                for (label, command) in [
                    ("Edit…", "activity.update"),
                    ("Progress…", "activity.progress"),
                    ("Assign resource…", "resource.assign"),
                    ("Add expense…", "expense.add"),
                    ("Delete…", "activity.delete"),
                ] {
                    if ui.button(label).clicked() {
                        self.open_command(command, &[("activity", &selected)]);
                    }
                }
                if ui.button("Add successor link…").clicked() {
                    self.open_command("relationship.add", &[("pred", &selected)]);
                }
            });
            if selected.is_empty() {
                ui.label(RichText::new("Click a row to select an activity").weak());
            } else {
                ui.label(RichText::new(format!("Selected: {selected}")).strong());
            }
        });
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.table.search)
                    .hint_text("Find code or name")
                    .desired_width(180.0),
            );
            egui::ComboBox::from_id_salt("table_show")
                .selected_text(
                    table::SHOWS
                        .iter()
                        .find(|s| s.0 == self.table.show)
                        .map_or("", |s| s.1),
                )
                .show_ui(ui, |ui| {
                    for (show, label) in table::SHOWS {
                        ui.selectable_value(&mut self.table.show, show, label);
                    }
                });
            ui.menu_button("Columns", |ui| {
                for (col, label, _) in table::COLUMNS {
                    let mut on = self.table.visible.contains(&col);
                    let fixed = matches!(col, table::Col::Code | table::Col::Name);
                    if ui
                        .add_enabled(!fixed, egui::Checkbox::new(&mut on, label))
                        .changed()
                    {
                        self.table.toggle(col);
                    }
                }
            });
            let shown = self.table.apply(&rows);
            ui.label(RichText::new(format!("{} of {}", shown.len(), rows.len())).weak());
            let name = self.project.clone().unwrap_or_else(|| "schedule".into());
            if ui.button("Export CSV…").clicked() {
                let text = table::csv(&self.table, &shown);
                self.requests.push(FileRequest::SaveText {
                    title: "Export table as CSV".into(),
                    file_name: format!("{name} activities.csv"),
                    extension: "csv".into(),
                    text,
                    open_after: false,
                });
            }
            if ui
                .button("Print / PDF…")
                .on_hover_text("Opens a printable page in your browser; print it or save it as PDF")
                .clicked()
            {
                let summary = self.cached(|c| &mut c.summary, "schedule.run", json!({}));
                let text = table::html(&name, &summary, &self.table, &shown);
                self.requests.push(FileRequest::SaveText {
                    title: "Save printable schedule".into(),
                    file_name: format!("{name} schedule.html"),
                    extension: "html".into(),
                    text,
                    open_after: true,
                });
            }
        });
        ui.add_space(4.0);
        let shown: Vec<Value> = self.table.apply(&rows).into_iter().cloned().collect();
        let cols = self.table.visible.clone();
        let (sort_col, desc) = self.table.sort;
        let mut sort_by = None;
        let critical = Tokens::for_ui(ui).critical;
        let path_float = longest_path_float(&rows);
        egui::ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new("acts")
                    .striped(true)
                    .spacing([18.0, 6.0])
                    .show(ui, |ui| {
                        for &c in &cols {
                            let arrow = match (c == sort_col, desc) {
                                (true, false) => " ▲",
                                (true, true) => " ▼",
                                _ => "",
                            };
                            let h = ui.add(
                                egui::Button::new(
                                    RichText::new(format!("{}{arrow}", table::header(c))).strong(),
                                )
                                .frame(false),
                            );
                            if h.on_hover_text("Sort").clicked() {
                                sort_by = Some(c);
                            }
                        }
                        ui.end_row();
                        for r in &shown {
                            let code = r["code"].as_str().unwrap_or("").to_owned();
                            let is_sel = code == selected;
                            for &c in &cols {
                                let text = table::cell(r, c);
                                if c == table::Col::Code {
                                    if ui.selectable_label(is_sel, &text).clicked() {
                                        self.edit_activity =
                                            if is_sel { String::new() } else { code.clone() };
                                    }
                                } else if c == table::Col::TotalFloat
                                    && on_longest_path(r, path_float)
                                {
                                    ui.label(RichText::new(text).color(critical));
                                } else if table::numeric(c) {
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| ui.label(text),
                                    );
                                } else {
                                    ui.label(text);
                                }
                            }
                            ui.end_row();
                        }
                    });
            });
        if let Some(c) = sort_by {
            self.table.sort_by(c);
        }
    }

    /// Opens the first dialog for `command` with some pickers preset.
    fn open_command(&mut self, command: &str, presets: &[(&str, &str)]) {
        let Some(spec) = forms::FORMS.iter().find(|f| f.command == command) else {
            return;
        };
        let lookups = self.lookups();
        let mut form = forms::OpenForm::new(spec, self.today, &lookups);
        for (k, v) in presets {
            form.preset(k, v);
        }
        self.form = Some(form);
    }

    /// UI-04: risk register; "What-if" realizes a risk in a fresh sandbox.
    fn risks(&mut self, ui: &mut Ui) {
        let v = self.cached(|c| &mut c.risks, "risk.list", json!({}));
        let risks = v["risks"].as_array().cloned().unwrap_or_default();
        ui.horizontal(|ui| {
            if ui.button("+ Add risk…").clicked() {
                self.open_command("risk.add", &[]);
            }
            if ui.button("Correlate durations…").clicked() {
                self.open_command("risk.correlate", &[]);
            }
        });
        ui.add_space(4.0);
        if risks.is_empty() {
            Self::empty_state(ui, "No risks yet", "Add one with + Add risk…");
            return;
        }
        egui::Grid::new("risks")
            .striped(true)
            .spacing([18.0, 6.0])
            .show(ui, |ui| {
                for h in [
                    "Code",
                    "Title",
                    "Status",
                    "Probability",
                    "Affects",
                    "Expected delay",
                    "",
                ] {
                    ui.label(RichText::new(h).strong());
                }
                ui.end_row();
                for r in &risks {
                    let code = r["code"].as_str().unwrap_or("").to_owned();
                    ui.label(&code);
                    ui.label(r["title"].as_str().unwrap_or(""));
                    ui.label(r["status"].as_str().unwrap_or(""));
                    let p = r["probability"]
                        .as_f64()
                        .or_else(|| r["probability"].as_str().and_then(|s| s.parse().ok()))
                        .unwrap_or(0.0);
                    ui.label(format!("{:.0} %", p * 100.0));
                    ui.label(
                        r["bindings"]
                            .as_array()
                            .map(|b| {
                                b.iter()
                                    .filter_map(Value::as_str)
                                    .map(|s| s.replace("wbs:", "WBS "))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            })
                            .unwrap_or_default(),
                    );
                    ui.label(r["expected_delay"].as_str().unwrap_or(""));
                    ui.horizontal(|ui| {
                        if ui.small_button("Edit…").clicked() {
                            self.open_command("risk.update", &[("risk", &code)]);
                        }
                        if ui.small_button("Mitigate…").clicked() {
                            self.open_command("mitigation.propose", &[("risk", &code)]);
                        }
                        if r["status"] != "realized"
                            && ui
                                .small_button("What-if")
                                .on_hover_text(
                                    "Copy the plan into a sandbox and let this risk happen there",
                                )
                                .clicked()
                        {
                            let name = format!("if-{}", code.to_lowercase());
                            if self
                                .exec("sandbox.create", json!({ "name": name }))
                                .is_some()
                            {
                                self.exec("risk.realize", json!({ "branch": name, "risk": code }));
                                self.branch = name;
                                self.tab = Tab::Sandboxes;
                                self.cache = Cache::default();
                            }
                        }
                    });
                    ui.end_row();
                }
            });
    }

    /// UI-04: Monte Carlo dashboard: P-values, histogram, risk tornado.
    fn simulation_view(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.add(
                egui::Slider::new(&mut self.iterations, 200..=20_000)
                    .text("iterations")
                    .logarithmic(true),
            );
            if ui.button("Run risk analysis").clicked() {
                let p = self.with_branch(json!({ "iterations": self.iterations }));
                self.exec_long("simulation.run", p);
            }
        });
        let Some(sim) = self.simulation.clone() else {
            ui.label(
                "Run an analysis to see P50/P80, the finish distribution and the risk tornado.",
            );
            return;
        };
        let f = &sim["finish"];
        ui.label(format!(
            "Finish without risks {}    P10 {}    P50 {}    P80 {}    P90 {}",
            short_date(&sim["deterministic_finish"]),
            short_date(&f["p10"]),
            short_date(&f["p50"]),
            short_date(&f["p80"]),
            short_date(&f["p90"]),
        ));
        ui.label(format!(
            "Cost P50 {}    P80 {}",
            money(&sim["cost"]["p50"]),
            money(&sim["cost"]["p80"])
        ));
        let tok = Tokens::for_ui(ui);
        let hist = sim["histogram"].as_array().cloned().unwrap_or_default();
        let peak = hist
            .iter()
            .filter_map(|h| h["count"].as_u64())
            .max()
            .unwrap_or(1)
            .max(1) as f32;
        let (resp, painter) =
            ui.allocate_painter(Vec2::new(ui.available_width(), 160.0), Sense::hover());
        let w = resp.rect.width() / hist.len().max(1) as f32;
        for (k, h) in hist.iter().enumerate() {
            let hgt = h["count"].as_u64().unwrap_or(0) as f32 / peak * (resp.rect.height() - 4.0);
            let r = Rect::from_min_max(
                Pos2::new(resp.rect.min.x + k as f32 * w, resp.rect.max.y - hgt),
                Pos2::new(
                    resp.rect.min.x + (k as f32 + 1.0) * w - 1.0,
                    resp.rect.max.y,
                ),
            );
            painter.rect_filled(r, 0.0, tok.normal);
        }
        ui.separator();
        ui.strong("Risk tornado (finish impact, days)");
        let tornado = sim["risk_tornado"].as_array().cloned().unwrap_or_default();
        let max = tornado
            .iter()
            .filter_map(|t| t["finish_impact_days"].as_f64())
            .fold(1.0_f64, |a, b| a.max(b.abs())) as f32;
        for t in &tornado {
            ui.horizontal(|ui| {
                ui.add_sized(
                    [60.0, ROW_H],
                    egui::Label::new(t["risk"].as_str().unwrap_or("")),
                );
                let d = t["finish_impact_days"].as_f64().unwrap_or(0.0) as f32;
                let (resp, painter) =
                    ui.allocate_painter(Vec2::new(300.0, ROW_H - 6.0), Sense::hover());
                let len = d.abs() / max * resp.rect.width();
                painter.rect_filled(
                    Rect::from_min_size(resp.rect.min, Vec2::new(len, resp.rect.height())),
                    2.0,
                    tok.critical,
                );
                ui.label(format!(
                    "{d:.1} d  (occurs {:.0} %)",
                    t["occurrence_rate"].as_f64().unwrap_or(0.0) * 100.0
                ));
            });
        }
    }

    /// UI-05: sandboxes, diff against live, human commit.
    fn sandboxes(&mut self, ui: &mut Ui) {
        self.proposals(ui);
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut self.new_sandbox).desired_width(140.0));
            if ui.button("New sandbox").clicked() {
                let name = self.new_sandbox.clone();
                if self
                    .exec("sandbox.create", json!({ "name": name }))
                    .is_some()
                {
                    self.branch = name;
                }
            }
        });
        if self.branch == "live" {
            ui.label("Open a branch above to compare it with live.");
            return;
        }
        let branch = self.branch.clone();
        let diff = self.cached(|c| &mut c.diff, "sandbox.diff", json!({ "head": branch }));
        let s = &diff["schedule"];
        ui.label(format!(
            "Finish: live {}, this sandbox {} ({}).   Newly critical: {}",
            long_date(&s["finish_base"]),
            long_date(&s["finish_head"]),
            s["finish_shift"].as_str().unwrap_or("-"),
            s["newly_critical"]
                .as_array()
                .map(|a| a
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", "))
                .unwrap_or_default(),
        ));
        egui::Grid::new("diff").striped(true).show(ui, |ui| {
            for c in diff["changes"].as_array().into_iter().flatten() {
                ui.label(c["change"].as_str().unwrap_or(""));
                ui.label(c["entity"].as_str().unwrap_or(""));
                ui.label(
                    c["fields"]
                        .as_array()
                        .map(|f| {
                            f.iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_default(),
                );
                ui.end_row();
            }
        });
        ui.horizontal(|ui| {
            if ui.button("Commit to live").clicked()
                && self
                    .exec("sandbox.commit", json!({ "sandbox": branch }))
                    .is_some()
            {
                self.branch = "live".into();
            }
            if ui.button("Discard").clicked()
                && self
                    .exec("sandbox.discard", json!({ "sandbox": branch }))
                    .is_some()
            {
                self.branch = "live".into();
            }
        });
    }

    /// UI-06: resource usage bars, overallocation, cost and EVM.
    fn resources(&mut self, ui: &mut Ui) {
        let data = match self.cache.resources.clone() {
            Some(d) => d,
            None => {
                let b = json!({ "branch": self.branch });
                let mut wp = b.clone();
                wp["period"] = json!("week");
                let d = json!({
                    "hist": self.exec("resource.histogram", wp).unwrap_or(Value::Null),
                    "over": self.exec("resource.overallocation", b.clone()).unwrap_or(Value::Null),
                    "cost": self.exec("cost.summary", b.clone()).unwrap_or(Value::Null),
                    "evm": self.exec("evm.report", b).unwrap_or(Value::Null),
                });
                self.cache.resources = Some(d.clone());
                d
            }
        };
        let tok = Tokens::for_ui(ui);
        // Buckets list only weeks with work, so bars are placed by date.
        let resources = data["hist"]["resources"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let day = |b: &Value| {
            b["from"]
                .as_str()
                .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
        };
        let all_days: Vec<chrono::NaiveDate> = resources
            .iter()
            .flat_map(|r| r["buckets"].as_array().cloned().unwrap_or_default())
            .filter_map(|b| day(&b))
            .collect();
        let (Some(first), Some(last)) = (all_days.iter().min(), all_days.iter().max()) else {
            ui.label(
                RichText::new("No resources assigned yet (Project > Assign resource…).").weak(),
            );
            return;
        };
        let weeks = ((*last - *first).num_days() / 7 + 1).max(1) as f32;
        let width = ui.available_width().min(1100.0) - 140.0;
        for r in &resources {
            let name = r["resource"].as_str().unwrap_or("");
            let bars: Vec<(chrono::NaiveDate, f32)> = r["buckets"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|b| {
                    let units = b["units"]
                        .as_str()
                        .and_then(|s| s.parse().ok())
                        .or_else(|| b["units"].as_f64())? as f32;
                    Some((day(b)?, units))
                })
                .collect();
            let peak = bars.iter().map(|b| b.1).fold(1.0_f32, f32::max);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_width(130.0);
                    ui.label(RichText::new(name).strong());
                    ui.label(
                        RichText::new(format!("peak {peak:.0} h/week"))
                            .weak()
                            .small(),
                    );
                });
                let (resp, painter) = ui.allocate_painter(Vec2::new(width, 64.0), Sense::hover());
                let rect = resp.rect;
                painter.line_segment(
                    [rect.left_bottom(), rect.right_bottom()],
                    Stroke::new(1.0, tok.grid),
                );
                let w = rect.width() / weeks;
                for (d, units) in &bars {
                    let k = (*d - *first).num_days() as f32 / 7.0;
                    let h = units / peak * (rect.height() - 4.0);
                    let bar = Rect::from_min_max(
                        Pos2::new(rect.min.x + k * w + 1.0, rect.max.y - h),
                        Pos2::new(rect.min.x + (k + 1.0) * w - 1.0, rect.max.y),
                    );
                    painter.rect_filled(bar, 2.0, tok.normal);
                    if resp
                        .hover_pos()
                        .is_some_and(|p| bar.expand2(Vec2::new(0.0, 60.0)).contains(p))
                    {
                        resp.clone().on_hover_text_at_pointer(format!(
                            "{name}: week of {} {} {}: {units:.0} hours",
                            d.format("%d"),
                            month(chrono::Datelike::month(d)),
                            chrono::Datelike::year(d)
                        ));
                    }
                }
            });
        }
        ui.horizontal(|ui| {
            ui.add_space(138.0);
            ui.label(
                RichText::new(format!(
                    "{} {} {}",
                    first.format("%d"),
                    month(chrono::Datelike::month(first)),
                    chrono::Datelike::year(first)
                ))
                .weak()
                .small(),
            );
            ui.add_space((width - 160.0).max(0.0));
            ui.label(
                RichText::new(format!(
                    "{} {} {}",
                    last.format("%d"),
                    month(chrono::Datelike::month(last)),
                    chrono::Datelike::year(last)
                ))
                .weak()
                .small(),
            );
        });
        let over = data["over"]["overallocations"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        ui.label(if over.is_empty() {
            RichText::new("No resource is overallocated.").color(tok.ok)
        } else {
            RichText::new(format!(
                "{} overallocation period(s): Tools > Level resources… can fix them.",
                over.len()
            ))
            .color(tok.critical)
        });
        ui.separator();
        ui.label(RichText::new(format!("Budget {}", money(&data["cost"]["total"]))).strong());
        let e = &data["evm"];
        if e.is_object() {
            let started = money(&e["pv"]) != "0" || money(&e["ac"]) != "0";
            if started {
                egui::Grid::new("evm").spacing([24.0, 4.0]).show(ui, |ui| {
                    for (k, label) in [
                        ("pv", "Planned value"),
                        ("ev", "Earned value"),
                        ("ac", "Actual cost"),
                        ("eac", "Estimate at completion"),
                    ] {
                        ui.label(RichText::new(label).weak());
                        ui.label(money(&e[k]));
                        ui.end_row();
                    }
                    for (k, label) in [
                        ("cpi", "Cost performance (CPI)"),
                        ("spi", "Schedule performance (SPI)"),
                    ] {
                        ui.label(RichText::new(label).weak());
                        ui.label(e[k].as_str().unwrap_or("–"));
                        ui.end_row();
                    }
                });
                ui.label(
                    RichText::new(format!(
                        "Against baseline {}",
                        e["baseline"].as_str().unwrap_or("–")
                    ))
                    .weak()
                    .small(),
                );
            } else {
                ui.label(
                    RichText::new(
                        "Earned value appears once progress is recorded (Project > Update progress…).",
                    )
                    .weak(),
                );
            }
        }
    }
}

/// Minutes since the engine epoch from an ISO-like date string.
/// A money amount with thousands separators: `654360.00` → `654,360`.
fn money(v: &Value) -> String {
    let n = v
        .as_f64()
        .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()));
    let Some(n) = n else {
        return "–".into();
    };
    let whole = format!("{:.0}", n.abs());
    let mut out = String::new();
    for (i, c) in whole.chars().enumerate() {
        if i > 0 && (whole.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0.0 { format!("-{out}") } else { out }
}

/// `2027-03-01T08:00` → `01 Mar 27` (P6 style); `–` when absent.
fn short_date(v: &Value) -> String {
    date_parts(v).map_or_else(
        || "–".into(),
        |(y, m, d, _)| format!("{d:02} {} {:02}", month(m), y % 100),
    )
}

/// `2027-03-01T08:00` → `01 Mar 2027 08:00`.
fn long_date(v: &Value) -> String {
    date_parts(v).map_or_else(
        || "–".into(),
        |(y, m, d, t)| format!("{d:02} {} {y} {t}", month(m)),
    )
}

fn month(m: u32) -> &'static str {
    MONTHS
        .get(m.saturating_sub(1) as usize)
        .copied()
        .unwrap_or("?")
}

fn date_parts(v: &Value) -> Option<(i32, u32, u32, String)> {
    use chrono::{Datelike, Timelike};
    let dt = chrono::NaiveDateTime::parse_from_str(v.as_str()?, "%Y-%m-%dT%H:%M").ok()?;
    Some((
        dt.year(),
        dt.month(),
        dt.day(),
        format!("{:02}:{:02}", dt.hour(), dt.minute()),
    ))
}

/// A duration cell: `5d`; a three-point estimate shows its most likely
/// value with `~` (the full range is in the Activities view).
fn duration_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Object(o) => format!("~{}", o.get("m").and_then(Value::as_str).unwrap_or("-")),
        _ => String::new(),
    }
}

/// Shortens `s` with an ellipsis to fit about `width` points at
/// `char_w` points per character.
fn clip(s: &str, width: f32, char_w: f32) -> String {
    let max = usize::try_from((width / char_w).max(1.0) as i64).unwrap_or(1);
    if s.chars().count() <= max {
        s.to_owned()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

/// Month row and day ticks above the bars, with vertical week lines.
#[allow(clippy::too_many_arguments)]
fn timeline_header(
    painter: &egui::Painter,
    x0: f32,
    y0: f32,
    header: f32,
    t0: i64,
    span_days: f32,
    ppd: f32,
    font: &FontId,
    text: Color32,
    weak: Color32,
    grid: Color32,
    height: f32,
) {
    use chrono::Datelike;
    let Some(start) = chrono::DateTime::from_timestamp(t0 * 60, 0).map(|d| d.date_naive()) else {
        return;
    };
    // Label every n-th week so labels never overlap.
    let every = ((44.0 / (7.0 * ppd)).ceil() as i64).max(1);
    let days = span_days as i64;
    let mut last_month = 0;
    for d in 0..=days {
        let Some(day) = start.checked_add_signed(chrono::Duration::days(d)) else {
            break;
        };
        let gx = x0 + d as f32 * ppd;
        if day.month() != last_month {
            last_month = day.month();
            // Long label when the month is wide enough, short or none otherwise.
            let month_w = 30.0 * ppd;
            let label = if month_w >= 72.0 {
                format!("{} {}", month(day.month()), day.year())
            } else if month_w >= 30.0 {
                month(day.month()).to_owned()
            } else {
                String::new()
            };
            painter.text(
                Pos2::new(gx + 4.0, y0 + 11.0),
                Align2::LEFT_CENTER,
                label,
                font.clone(),
                text,
            );
            painter.line_segment(
                [Pos2::new(gx, y0), Pos2::new(gx, y0 + header)],
                Stroke::new(1.0, grid),
            );
        }
        if day.weekday() == chrono::Weekday::Mon && (d / 7) % every == 0 {
            painter.text(
                Pos2::new(gx + 2.0, y0 + header - 12.0),
                Align2::LEFT_CENTER,
                format!("{:02}", day.day()),
                font.clone(),
                weak,
            );
            painter.line_segment(
                [Pos2::new(gx, y0 + header), Pos2::new(gx, y0 + height)],
                Stroke::new(1.0, grid),
            );
        }
    }
}

/// Colour key of the Gantt toolbar.
fn legend(ui: &mut Ui, tok: &Tokens) {
    // Drawn right to left, so the last entry shows first.
    for (c, label, diamond) in [
        (tok.milestone, "Milestone", true),
        (tok.critical, "Longest path", false),
        (tok.normal, "Activity", false),
    ] {
        ui.label(RichText::new(label).small().color(tok.muted));
        let (r, p) = ui.allocate_painter(Vec2::new(14.0, 10.0), Sense::hover());
        if diamond {
            let m = r.rect.center();
            p.add(egui::Shape::convex_polygon(
                vec![
                    m + Vec2::new(0.0, -4.5),
                    m + Vec2::new(4.5, 0.0),
                    m + Vec2::new(0.0, 4.5),
                    m + Vec2::new(-4.5, 0.0),
                ],
                c,
                Stroke::NONE,
            ));
        } else {
            p.rect_filled(r.rect.shrink2(Vec2::new(0.0, 2.0)), 3.0, c);
        }
        ui.add_space(6.0);
    }
}

/// One line of the Gantt chart: an activity or a summary (project, WBS).
#[derive(Debug, Clone, Default)]
struct Line {
    code: String,
    name: String,
    /// Indent: 0 project or ungrouped, 1 WBS, 2 activity under a WBS.
    level: u8,
    summary: bool,
    milestone: bool,
    on_path: bool,
    dur: String,
    float: String,
    /// Early start and finish as shown (`YYYY-MM-DDTHH:MM`).
    start: Value,
    finish: Value,
    late_finish: Value,
    es: Option<i64>,
    ef: Option<i64>,
    lf: Option<i64>,
    /// Latest baseline's dates, when there is one.
    baseline_start: Value,
    baseline_finish: Value,
    bs: Option<i64>,
    bf: Option<i64>,
    /// Finish variance against the baseline (`2d` = later).
    variance: String,
    /// The compared plan's early start and finish of the same code.
    cs: Option<i64>,
    cf: Option<i64>,
}

impl Line {
    /// The line of one `schedule.activities` row.
    fn activity(r: &Value, level: u8, path_float: Option<f64>) -> Self {
        Self {
            code: r["code"].as_str().unwrap_or("").to_owned(),
            name: r["name"].as_str().unwrap_or("").to_owned(),
            level,
            summary: false,
            milestone: r["kind"].as_str().unwrap_or("").ends_with("milestone"),
            on_path: on_longest_path(r, path_float),
            dur: duration_text(&r["duration"]),
            float: r["total_float"].as_str().unwrap_or("–").to_owned(),
            start: r["early_start"].clone(),
            finish: r["early_finish"].clone(),
            late_finish: r["late_finish"].clone(),
            es: minutes(&r["early_start"]),
            ef: minutes(&r["early_finish"]),
            lf: minutes(&r["late_finish"]),
            baseline_start: r["baseline_start"].clone(),
            baseline_finish: r["baseline_finish"].clone(),
            bs: minutes(&r["baseline_start"]),
            bf: minutes(&r["baseline_finish"]),
            variance: r["finish_variance"].as_str().unwrap_or("–").to_owned(),
            cs: None,
            cf: None,
        }
    }

    /// A roll-up of `members`: earliest start, latest finish, least float.
    fn summary(code: &str, name: &str, level: u8, members: &[Line]) -> Self {
        let first = members
            .iter()
            .filter(|m| m.es.is_some())
            .min_by_key(|m| m.es);
        let last = members
            .iter()
            .filter(|m| m.ef.is_some())
            .max_by_key(|m| m.ef);
        let float = members
            .iter()
            .filter(|m| !m.summary)
            .filter_map(|m| {
                m.float
                    .trim_end_matches('d')
                    .parse::<f64>()
                    .ok()
                    .map(|f| (f, &m.float))
            })
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map_or("–".to_owned(), |f| f.1.clone());
        Self {
            code: code.to_owned(),
            name: name.to_owned(),
            level,
            summary: true,
            on_path: members.iter().any(|m| m.on_path),
            float,
            start: first.map_or(Value::Null, |m| m.start.clone()),
            finish: last.map_or(Value::Null, |m| m.finish.clone()),
            es: first.and_then(|m| m.es),
            ef: last.and_then(|m| m.ef),
            ..Self::default()
        }
    }
}

/// The Gantt lines for `rows`: flat, or (grouped) a project summary, then
/// each WBS element in order of its first start with its activities.
fn gantt_lines(rows: &[Value], lookups: &Value, project: &str, group: bool) -> Vec<Line> {
    let path_float = longest_path_float(rows);
    if !group {
        return rows
            .iter()
            .map(|r| Line::activity(r, 0, path_float))
            .collect();
    }
    let wbs_name = |code: &str| {
        lookups["wbs"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|w| w["code"] == code)
            .and_then(|w| w["name"].as_str())
            .unwrap_or(if code.is_empty() {
                "Not in a WBS"
            } else {
                code
            })
            .to_owned()
    };
    let mut groups: Vec<(String, Vec<Line>)> = Vec::new();
    for r in rows {
        let wbs = r["wbs"].as_str().unwrap_or("").to_owned();
        let line = Line::activity(r, 2, path_float);
        match groups.iter_mut().find(|g| g.0 == wbs) {
            Some(g) => g.1.push(line),
            None => groups.push((wbs, vec![line])),
        }
    }
    groups.sort_by_key(|g| (g.1.iter().filter_map(|l| l.es).min(), g.0.clone()));
    let all: Vec<Line> = groups.iter().flat_map(|g| g.1.clone()).collect();
    let mut out = vec![Line::summary("", project, 0, &all)];
    for (code, members) in groups {
        out.push(Line::summary(&code, &wbs_name(&code), 1, &members));
        out.extend(members);
    }
    out
}

/// The least total float in days among `rows` (the longest path's float),
/// or `None` when no row has one.
fn longest_path_float(rows: &[Value]) -> Option<f64> {
    rows.iter()
        .filter_map(|r| r["total_float_days"].as_f64())
        .min_by(f64::total_cmp)
}

/// Whether a row is on the longest path: critical, or with the least total
/// float of the project (so a deadline that gives every activity float
/// still shows which chain drives the finish).
fn on_longest_path(r: &Value, path_float: Option<f64>) -> bool {
    r["critical"] == true
        || r["total_float_days"]
            .as_f64()
            .zip(path_float)
            .is_some_and(|(f, p)| (f - p).abs() < 1e-9)
}

/// `2027-03-01T08:00` → `01 Mar 2027`; `–` when absent.
fn day_date(v: &Value) -> String {
    date_parts(v).map_or_else(
        || "–".into(),
        |(y, m, d, _)| format!("{d:02} {} {y}", month(m)),
    )
}

/// Inverse of [`minutes`]: `YYYY-MM-DDTHH:MM` as JSON (null if out of range).
fn iso(minutes: i64) -> Value {
    json!(
        chrono::DateTime::from_timestamp(minutes.saturating_mul(60), 0)
            .map(|d| d.format("%Y-%m-%dT%H:%M").to_string())
    )
}

fn minutes(v: &Value) -> Option<i64> {
    let dt = chrono::NaiveDateTime::parse_from_str(v.as_str()?, "%Y-%m-%dT%H:%M").ok()?;
    Some(dt.and_utc().timestamp() / 60)
}

#[cfg(test)]
mod tests;
