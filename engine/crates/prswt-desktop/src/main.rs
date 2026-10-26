//! PRSWT desktop app (UI-01, DSK-02..04): an eframe window around
//! [`prswt_ui::PrswtApp`]. Local only, no sign-in (ADR-0009).
//!
//! ```text
//! prswt [FILE.prswt] [--sample bridge|turnaround] [--view TAB] [--control PORT]
//!       [--theme light|dark] [--select CODE] [--cloud-open PROJECT_ID]
//!       [--screenshot OUT.bmp]
//! ```
//! `--screenshot` saves the window as a BMP once it has settled and quits
//! (documentation and design review; it works where screen capture does not).
//! The shell owns everything that touches the operating system: native file
//! dialogs, reading and atomically writing files ([`files`]), the window
//! title with its unsaved-changes mark, the "save changes?" question before
//! closing, and settings kept by eframe in the per-user app-data folder
//! (window size, theme, recent files).
//!
//! Cloud projects (ADR-0010, [`cloud`]): signed-in people open, share and
//! sync projects kept on the server; a worker thread does the network and
//! the window stays responsive offline.
//!
//! With `--control`, the same workspace is served on `127.0.0.1:PORT`, so an
//! agent can drive what the human is looking at:
//! `prswt-cli mcp --connect PORT`.
// Release builds are GUI apps on Windows: no console window next to them.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
#![allow(clippy::print_stderr)]

mod cloud;
mod files;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use prswt_app::{Clock, Workspace};
use prswt_core::Instant;
use prswt_ui::{FileRequest, Format, PrswtApp};

/// Extension of PRSWT project files.
const EXT: &str = "prswt";
/// Settings key of the recent-files list (JSON array of paths).
const RECENT_KEY: &str = "recent_files";
/// Settings key of the signed-in cloud account (email; the token is in the
/// OS keychain).
const ACCOUNT_KEY: &str = "cloud_account";
/// Seconds after a local change before it is synced (edits come in bursts).
const SYNC_DELAY_SECS: f64 = 1.5;
/// Seconds between background syncs of open cloud projects (catch-up after
/// being offline; live changes arrive sooner through the watch stream).
const SYNC_EVERY_SECS: f64 = 60.0;

/// Wall clock injected into the engine (UTC minutes since 2000-01-01).
struct SystemClock;

impl Clock for SystemClock {
    #[allow(clippy::disallowed_methods)]
    fn now(&self) -> Instant {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        Instant((i64::try_from(secs).unwrap_or(0) - 946_684_800) / 60)
    }
}

/// Where an open project lives on disk and the revision last saved there.
#[derive(Debug, Default, Clone)]
struct Doc {
    path: Option<PathBuf>,
    saved: Option<u64>,
}

/// Something waiting for the "save changes?" answer.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pending {
    /// Close this project.
    Close(String),
    /// Quit; projects already answered "don't save" are skipped.
    Quit,
}

/// eframe adapter: forwards frames to the UI and serves its file requests.
struct Window {
    app: PrswtApp,
    /// Project id → its file.
    docs: HashMap<String, Doc>,
    pending: Option<Pending>,
    /// Projects whose changes the user chose to drop while quitting.
    discarded: HashSet<String>,
    /// Set once quitting is confirmed, so the next close request goes through.
    closing: bool,
    title: String,
    /// `--screenshot`: where to save the window, frames drawn so far, the
    /// slot the capture callback fills, and whether the capture was asked.
    shot: Option<(PathBuf, u32, Captured, bool)>,
    /// The cloud worker.
    cloud: cloud::Client,
    /// The engine (shared with the UI and the cloud worker).
    ws: Arc<Mutex<Workspace>>,
    /// Folder of cached cloud copies.
    cache: PathBuf,
    /// Cloud project id → (revision last seen, when to sync it next).
    watched: HashMap<String, (Option<u64>, Option<f64>)>,
    /// When all cloud projects are synced next (seconds of window time).
    next_round: f64,
    /// Theme preference last sent to the account settings.
    theme_sent: Option<egui::ThemePreference>,
    /// Change counter of each open project, read once per frame (the
    /// title, the unsaved mark and the sync schedule all need it).
    revs: HashMap<String, u64>,
    /// When the sync schedule was last checked (seconds of window time).
    scheduled_at: f64,
}

/// Where the screenshot callback leaves the captured image.
type Captured = Arc<Mutex<Option<Arc<egui::ColorImage>>>>;

/// Frames to draw before a `--screenshot` (fonts, layout and data settle).
const SHOT_AFTER_FRAMES: u32 = 12;
/// Most frames a `--screenshot` waits for cloud syncs (about a minute).
const SHOT_WAIT_FRAMES: u32 = 600;

impl eframe::App for Window {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        if ui.ctx().input(|i| i.viewport().close_requested()) && !self.closing {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.pending = Some(Pending::Quit);
        }
        for path in ui.ctx().input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect::<Vec<_>>()
        }) {
            self.open_path(&path);
        }
        self.read_revisions();
        self.cloud_events(ui.ctx());
        if let Some((id, _)) = self.app.selected() {
            self.app.unsaved = self.is_dirty(&id);
        }
        self.app.ui(ui);
        for r in self.app.take_requests() {
            self.handle(ui.ctx(), frame, r);
        }
        for r in self.app.take_cloud_requests() {
            self.cloud_request(r);
        }
        self.cloud_schedule(ui.ctx());
        self.confirm(ui.ctx(), frame);
        self.update_title(ui.ctx());
        self.screenshot(ui.ctx());
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if let Ok(s) = serde_json::to_string(&self.app.recent) {
            storage.set_string(RECENT_KEY, s);
        }
        storage.set_string(
            ACCOUNT_KEY,
            self.app.cloud.account.clone().unwrap_or_default(),
        );
        // Unsent cloud work survives a restart in the cache.
        for id in self.app.cloud.sync.keys().cloned().collect::<Vec<_>>() {
            self.cache_cloud(&id);
        }
    }
}

impl Window {
    /// Runs one File-menu request.
    fn handle(&mut self, ctx: &egui::Context, frame: &eframe::Frame, r: FileRequest) {
        match r {
            FileRequest::Open => {
                if let Some(p) = dialog(frame)
                    .add_filter("PRSWT project", &[EXT])
                    .pick_file()
                {
                    self.open_path(&p);
                }
            }
            FileRequest::OpenRecent(p) => self.open_path(Path::new(&p)),
            FileRequest::Save => {
                // A cloud project is saved by syncing it (and its cache).
                match self.app.selected() {
                    Some((id, _)) if self.app.cloud.sync.contains_key(&id) => {
                        self.cache_cloud(&id);
                        self.cloud.send(cloud::Job::Sync(id));
                    }
                    _ => {
                        self.save_selected(frame, false);
                    }
                }
            }
            FileRequest::SaveAs => {
                self.save_selected(frame, true);
            }
            FileRequest::Import(f) => self.import(frame, f),
            FileRequest::Export(f) => self.export(frame, f),
            FileRequest::Close => {
                if let Some((id, _)) = self.app.selected() {
                    if self.is_dirty(&id) {
                        self.pending = Some(Pending::Close(id));
                    } else {
                        self.close(&id);
                    }
                }
            }
            FileRequest::SaveText {
                title,
                file_name,
                extension,
                text,
                open_after,
            } => {
                let Some(path) = dialog(frame)
                    .set_title(&title)
                    .add_filter(extension.to_uppercase(), &[extension.as_str()])
                    .set_file_name(safe_file_name(&file_name))
                    .save_file()
                else {
                    return;
                };
                let path = files::with_extension(path, &extension);
                self.app.status = match files::write_atomic(&path, &text) {
                    Ok(()) => {
                        if open_after && open::that_detached(&path).is_err() {
                            format!("Saved {} (open it to print)", path.display())
                        } else {
                            format!("Saved {}", path.display())
                        }
                    }
                    Err(e) => format!("Not saved: {e}"),
                };
            }
            FileRequest::Quit => {
                self.pending = Some(Pending::Quit);
                // With nothing unsaved, `confirm` lets the close through at once.
                self.confirm(ctx, frame);
            }
        }
    }

    /// Opens a `.prswt` file, or imports XER / MSPDI by extension.
    fn open_path(&mut self, path: &Path) {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase);
        let format = Format::ALL
            .into_iter()
            .find(|f| ext.as_deref() == Some(f.extension()));
        if let Some(f) = format {
            self.import_path(f, path);
            return;
        }
        if let Some((id, _)) = self
            .docs
            .iter()
            .find(|(_, d)| d.path.as_deref() == Some(path))
        {
            let id = id.clone();
            self.app.select_id(&id);
            return;
        }
        let text = match files::read_text(path, prswt_state::MAX_FILE_BYTES) {
            Ok(t) => t,
            Err(e) => {
                self.app.status = e;
                return;
            }
        };
        if let Some(id) = self.app.open_text("file.open_project", &text) {
            let saved = self.app.revision(&id);
            if let Some(r) = saved {
                self.revs.insert(id.clone(), r);
            }
            self.docs.insert(
                id,
                Doc {
                    path: Some(path.to_owned()),
                    saved,
                },
            );
            files::push_recent(&mut self.app.recent, path);
            self.app.status = format!("Opened {}", path.display());
        }
    }

    /// Saves the selected project; `ask` (or no path yet) shows the dialog.
    /// Returns whether it was written.
    fn save_selected(&mut self, frame: &eframe::Frame, ask: bool) -> bool {
        let Some((id, name)) = self.app.selected() else {
            return false;
        };
        let known = self.docs.get(&id).and_then(|d| d.path.clone());
        let path = match known {
            Some(p) if !ask => p,
            _ => match dialog(frame)
                .add_filter("PRSWT project", &[EXT])
                .set_file_name(format!("{}.{EXT}", safe_file_name(&name)))
                .save_file()
            {
                Some(p) => files::with_extension(p, EXT),
                None => return false,
            },
        };
        let rev = self.app.revision(&id);
        if let Some(r) = rev {
            self.revs.insert(id.clone(), r);
        }
        let Some(text) = self.app.project_text("file.save_project") else {
            return false;
        };
        match files::write_atomic(&path, &text) {
            Ok(()) => {
                files::push_recent(&mut self.app.recent, &path);
                self.app.status = format!("Saved {}", path.display());
                self.docs.insert(
                    id,
                    Doc {
                        path: Some(path),
                        saved: rev,
                    },
                );
                true
            }
            Err(e) => {
                self.app.status = format!("Not saved: {e}");
                false
            }
        }
    }

    fn import(&mut self, frame: &eframe::Frame, f: Format) {
        if let Some(p) = dialog(frame)
            .add_filter(f.label(), &[f.extension()])
            .pick_file()
        {
            self.import_path(f, &p);
        }
    }

    /// Imports a foreign file as a new, not yet saved project.
    fn import_path(&mut self, f: Format, path: &Path) {
        match files::read_text(path, prswt_state::MAX_FILE_BYTES) {
            Ok(text) => {
                if let Some(id) = self.app.open_text(f.import_command(), &text) {
                    self.docs.insert(id, Doc::default());
                    self.app.status =
                        format!("Imported {} (not saved yet: File ▸ Save)", path.display());
                }
            }
            Err(e) => self.app.status = e,
        }
    }

    fn export(&mut self, frame: &eframe::Frame, f: Format) {
        let Some((_, name)) = self.app.selected() else {
            return;
        };
        let Some(path) = dialog(frame)
            .add_filter(f.label(), &[f.extension()])
            .set_file_name(format!("{}.{}", safe_file_name(&name), f.extension()))
            .save_file()
        else {
            return;
        };
        let path = files::with_extension(path, f.extension());
        if let Some(text) = self.app.project_text(f.export_command()) {
            self.app.status = match files::write_atomic(&path, &text) {
                Ok(()) => format!("Exported {}", path.display()),
                Err(e) => format!("Not exported: {e}"),
            };
        }
    }

    fn close(&mut self, id: &str) {
        let cloud = self.app.cloud.sync.contains_key(id);
        if cloud {
            // Unsent work stays in the cache for the next open.
            self.cache_cloud(id);
        }
        if self.app.close_project(id) {
            self.docs.remove(id);
            self.discarded.remove(id);
            if cloud {
                self.forget_cloud(id);
            }
        }
    }

    /// A cloud project left the window: no sync state, no stream.
    fn forget_cloud(&mut self, id: &str) {
        self.app.cloud.sync.remove(id);
        self.watched.remove(id);
        self.cloud.send(cloud::Job::Unwatch(id.to_owned()));
    }

    /// Reads every open project's change counter (one engine call per
    /// frame). While the cloud worker holds the engine, the last values
    /// stay, so the window never waits for the network side.
    fn read_revisions(&mut self) {
        let Ok(mut w) = self.ws.try_lock() else {
            return;
        };
        let Ok(v) = w.execute(
            &prswt_app::Ctx::human("desktop"),
            "project.list",
            &serde_json::json!({ "brief": true }),
        ) else {
            return;
        };
        self.revs = v["projects"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| Some((p["project"].as_str()?.to_owned(), p["revision"].as_u64()?)))
            .collect();
    }

    /// Change counter of an open project as of this frame.
    fn rev(&self, id: &str) -> Option<u64> {
        self.revs.get(id).copied()
    }

    /// `--screenshot`: asks for a capture after a few frames, saves it as
    /// BMP when it arrives and closes the window.
    fn screenshot(&mut self, ctx: &egui::Context) {
        let Some((path, frames, slot, asked)) = &mut self.shot else {
            return;
        };
        let syncing = self
            .app
            .cloud
            .sync
            .values()
            .any(|s| s.text.starts_with("Syncing"))
            || self.app.cloud.busy.is_some();
        *frames = frames.saturating_add(1);
        // Cloud syncs settle first (bounded), then one capture is asked for.
        let settled = !syncing || *frames >= SHOT_WAIT_FRAMES;
        if settled && !*asked && *frames >= SHOT_AFTER_FRAMES {
            *asked = true;
            let slot = slot.clone();
            ctx.request_screenshot(move |image| {
                if let Ok(mut s) = slot.lock() {
                    *s = Some(image);
                }
            });
        }
        if !*asked {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let image = slot.lock().ok().and_then(|mut s| s.take());
        if let Some(image) = image {
            let [w, h] = image.size;
            let rgba: Vec<u8> = image
                .pixels
                .iter()
                .flat_map(egui::Color32::to_array)
                .collect();
            match files::bmp(w, h, &rgba).map(|b| std::fs::write(&*path, b)) {
                Some(Ok(())) => {}
                Some(Err(e)) => eprintln!("screenshot: {e}"),
                None => eprintln!("screenshot: bad image size"),
            }
            self.closing = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ctx.request_repaint();
    }

    /// Applies what the cloud worker reported.
    fn cloud_events(&mut self, ctx: &egui::Context) {
        use cloud::Done;
        let mut any = false;
        while let Ok(d) = self.cloud.done.try_recv() {
            any = true;
            let c = &mut self.app.cloud;
            match d {
                Done::SignedIn(email, settings) => {
                    c.account = Some(email.clone());
                    c.error = None;
                    self.app.status = format!("Signed in as {email}");
                    let theme = match settings["theme"].as_str() {
                        Some("dark") => Some(egui::ThemePreference::Dark),
                        Some("light") => Some(egui::ThemePreference::Light),
                        Some("system") => Some(egui::ThemePreference::System),
                        _ => None,
                    };
                    if let Some(t) = theme {
                        ctx.set_theme(t);
                        self.theme_sent = Some(t);
                    }
                }
                Done::SignedOut => {
                    c.account = None;
                    self.app.status = "Signed out of the cloud".into();
                }
                Done::Busy(b) => c.busy = b,
                Done::Projects(rows) => {
                    c.error = None;
                    c.projects = rows
                        .iter()
                        .map(|p| prswt_ui::CloudProject {
                            id: p["project"].as_str().unwrap_or("").to_owned(),
                            name: p["name"].as_str().unwrap_or("").to_owned(),
                            role: p["role"].as_str().unwrap_or("viewer").to_owned(),
                        })
                        .collect();
                }
                Done::Opened(id) => {
                    c.sync.entry(id.clone()).or_default().text = "Syncing…".into();
                    self.watched.insert(id.clone(), (None, None));
                    self.app.select_id(&id);
                }
                Done::Synced(id, pending, conflicts, sent) => {
                    if !c.sync.contains_key(&id) {
                        continue; // closed meanwhile
                    }
                    let st = c.sync.entry(id.clone()).or_default();
                    st.problem = false;
                    st.text = if pending > 0 {
                        format!("Synced · {pending} branch(es) still sending")
                    } else {
                        "Synced with the cloud".into()
                    };
                    if !conflicts.is_empty() {
                        self.app.status = format!(
                            "{} change(s) collided with someone else's; theirs were kept",
                            conflicts.len()
                        );
                    }
                    st.conflicts.extend(conflicts);
                    // The revision the worker sent: edits made during the
                    // sync differ from it and are sent shortly.
                    self.watched.insert(id, (sent, None));
                    self.app.refresh();
                }
                Done::Offline(id, why) => {
                    if let Some(st) = c.sync.get_mut(&id) {
                        st.problem = true;
                        st.text = "Offline · changes kept on this computer".into();
                    }
                    self.app.status = why;
                }
                Done::Members(id, list) => {
                    // Only for the project whose share dialog is open.
                    if self.app.selected().is_some_and(|(sel, _)| sel == id) {
                        self.app.cloud.members = list;
                    }
                }
                // Reports about projects no longer open are dropped.
                Done::Present(id, people) => {
                    if let Some(st) = c.sync.get_mut(&id) {
                        st.present = people;
                    }
                }
                Done::Changed(id, who) => {
                    if let Some(st) = c.sync.get_mut(&id) {
                        st.last_change = Some(format!("Updated by {who}"));
                    }
                }
                Done::Error(e) => {
                    c.busy = None;
                    // A failed open or upload leaves no "Syncing…" behind.
                    for st in c.sync.values_mut() {
                        if st.text.starts_with("Syncing") {
                            st.text = "Not synced".into();
                            st.problem = true;
                        }
                    }
                    c.error = Some(e.clone());
                    self.app.status = e;
                }
            }
        }
        if any {
            ctx.request_repaint();
        }
        // The cloud worker reports asynchronously; look again soon.
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }

    /// Hands a cloud request from the UI to the worker.
    fn cloud_request(&mut self, r: prswt_ui::CloudRequest) {
        use cloud::Job;
        use prswt_ui::CloudRequest as R;
        self.app.cloud.error = None;
        let job = match r {
            R::SignIn => Job::SignIn,
            R::SignOut => Job::SignOut,
            R::ListProjects => Job::List,
            R::Upload(id) => Job::Upload(id),
            R::Open(id) => Job::Open(id),
            R::Sync(id) => Job::Sync(id),
            R::Members(id) => Job::Members(id),
            R::Invite(id, email, role) => Job::Invite(id, email, role),
            R::Remove(id, email) => Job::Remove(id, email),
        };
        self.cloud.send(job);
    }

    /// Syncs cloud projects shortly after local edits and every minute;
    /// sends a changed theme to the account settings.
    fn cloud_schedule(&mut self, ctx: &egui::Context) {
        if self.app.cloud.account.is_none() {
            return;
        }
        let now = ctx.input(|i| i.time);
        // A few checks a second are plenty (edits wait 1.5 s anyway).
        if now - self.scheduled_at < 0.25 && now >= self.scheduled_at {
            return;
        }
        self.scheduled_at = now;
        let round = now >= self.next_round;
        if round {
            self.next_round = now + SYNC_EVERY_SECS;
        }
        let ids: Vec<String> = self.watched.keys().cloned().collect();
        for id in ids {
            let rev = self.rev(&id);
            let Some((seen, due)) = self.watched.get(&id).copied() else {
                continue;
            };
            if rev.is_none() && self.app.revision(&id).is_none() {
                // Closed (by an agent, say): forget it everywhere.
                self.forget_cloud(&id);
                continue;
            }
            let due = if rev != seen && due.is_none() {
                Some(now + SYNC_DELAY_SECS)
            } else {
                due
            };
            let fire = round || due.is_some_and(|d| now >= d);
            if fire {
                self.app.cloud.sync.entry(id.clone()).or_default().text = "Syncing…".into();
                self.cloud.send(cloud::Job::Sync(id.clone()));
                self.watched.insert(id, (rev, None));
            } else {
                self.watched.insert(id, (seen, due));
            }
        }
        let theme = ctx.options(|o| o.theme_preference);
        if self.theme_sent != Some(theme) {
            self.theme_sent = Some(theme);
            let name = match theme {
                egui::ThemePreference::Dark => "dark",
                egui::ThemePreference::Light => "light",
                egui::ThemePreference::System => "system",
            };
            self.cloud
                .send(cloud::Job::Settings(serde_json::json!({ "theme": name })));
        }
    }

    /// Writes a cloud project's local copy (with its sync state) to the cache.
    fn cache_cloud(&mut self, id: &str) {
        let text = self.ws.lock().ok().and_then(|mut w| {
            w.execute(
                &prswt_app::Ctx::human("desktop"),
                "file.save_project",
                &serde_json::json!({ "project": id, "with_sync": true }),
            )
            .ok()
        });
        if let Some(t) = text.and_then(|v| v["text"].as_str().map(str::to_owned)) {
            let safe: String = id
                .chars()
                .filter(|c| c.is_ascii_hexdigit() || *c == '-')
                .collect();
            let written = std::fs::create_dir_all(&self.cache)
                .map_err(|e| e.to_string())
                .and_then(|()| files::write_atomic(&self.cache.join(format!("{safe}.prswt")), &t));
            if let Err(e) = written {
                self.app.status = format!("Could not keep the cloud copy on this computer: {e}");
            }
        }
    }

    /// Unsaved changes: never saved, or edited since the last save.
    fn is_dirty(&mut self, id: &str) -> bool {
        // Cloud copies are kept in the cache and synced; never "unsaved".
        if self.app.cloud.sync.contains_key(id) {
            return false;
        }
        let saved = self.docs.get(id).and_then(|d| d.saved);
        saved.is_none() || self.rev(id) != saved
    }

    /// Shows the "save changes?" dialog for the pending close or quit, or
    /// finishes it when nothing is unsaved.
    fn confirm(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let Some(pending) = self.pending.clone() else {
            return;
        };
        let target = match &pending {
            Pending::Close(id) => Some(id.clone()).filter(|id| self.is_dirty(id)),
            Pending::Quit => {
                let ids: Vec<String> = self.app.open_projects().into_iter().map(|p| p.0).collect();
                ids.into_iter()
                    .find(|id| !self.discarded.contains(id) && self.is_dirty(id))
            }
        };
        let Some(id) = target else {
            self.finish(ctx, &pending);
            return;
        };
        let name = self
            .app
            .open_projects()
            .into_iter()
            .find(|p| p.0 == id)
            .map_or_else(|| "project".to_owned(), |p| p.1);
        let mut answer = None;
        egui::Modal::new(egui::Id::unique("unsaved")).show(ctx, |ui| {
            ui.heading("Unsaved changes");
            ui.label(format!("Save changes to “{name}” before closing?"));
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    answer = Some(0);
                }
                if ui.button("Don't save").clicked() {
                    answer = Some(1);
                }
                if ui.button("Cancel").clicked() {
                    answer = Some(2);
                }
            });
        });
        match answer {
            Some(0) => {
                self.app.select_id(&id);
                if !self.save_selected(frame, false) {
                    self.pending = None;
                }
            }
            Some(1) => match pending {
                Pending::Close(_) => {
                    self.close(&id);
                    self.pending = None;
                }
                Pending::Quit => {
                    self.discarded.insert(id);
                }
            },
            Some(_) => {
                self.pending = None;
                self.discarded.clear();
            }
            None => {}
        }
    }

    fn finish(&mut self, ctx: &egui::Context, pending: &Pending) {
        self.pending = None;
        match pending {
            Pending::Close(id) => self.close(id),
            Pending::Quit => {
                self.closing = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    /// "name • — PRSWT", with • while there are unsaved changes.
    fn update_title(&mut self, ctx: &egui::Context) {
        let title = match self.app.selected() {
            Some((id, name)) => {
                let file = self
                    .docs
                    .get(&id)
                    .and_then(|d| d.path.as_ref())
                    .and_then(|p| p.file_name())
                    .map(|f| format!(" ({})", f.to_string_lossy()))
                    .unwrap_or_default();
                let mark = if self.is_dirty(&id) { " •" } else { "" };
                format!("{name}{file}{mark} — PRSWT")
            }
            None => "PRSWT — project controls & risk".to_owned(),
        };
        if title != self.title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }
}

/// The window / taskbar icon, drawn in code (no image files to ship): a
/// rounded blue tile with three Gantt bars, the last one critical red.
fn app_icon() -> egui::IconData {
    const N: u32 = 64;
    let mut rgba = Vec::with_capacity((N * N * 4) as usize);
    // (row band, start x, end x, colour)
    let bars: [(u32, u32, u32, [u8; 3]); 3] = [
        (16, 12, 36, [255, 255, 255]),
        (29, 24, 48, [255, 255, 255]),
        (42, 34, 54, [235, 80, 70]),
    ];
    for y in 0..N {
        for x in 0..N {
            // Rounded corners: radius 12.
            let (cx, cy) = (x.clamp(12, N - 13), y.clamp(12, N - 13));
            let (dx, dy) = (x.abs_diff(cx), y.abs_diff(cy));
            let inside = dx * dx + dy * dy <= 144;
            let mut px = if inside {
                [36, 92, 196, 255]
            } else {
                [0, 0, 0, 0]
            };
            for (top, x0, x1, c) in bars {
                if inside && (top..top + 8).contains(&y) && (x0..x1).contains(&x) {
                    px = [c[0], c[1], c[2], 255];
                }
            }
            rgba.extend_from_slice(&px);
        }
    }
    egui::IconData {
        rgba,
        width: N,
        height: N,
    }
}

/// A native file dialog owned by the app window, so it opens in front of
/// it (without a parent, Windows may show it behind the window).
fn dialog(frame: &eframe::Frame) -> rfd::FileDialog {
    rfd::FileDialog::new().set_parent(frame)
}

/// Today's UTC date from the system clock (the UI crate reads no clock).
fn today() -> jiff::civil::Date {
    let minutes = SystemClock.now().0;
    let days = minutes.div_euclid(1440);
    jiff::civil::date(2000, 1, 1)
        .checked_add(jiff::Span::new().days(days))
        .unwrap_or(jiff::civil::date(2000, 1, 1))
}

/// A project name made safe as a default file name on every OS.
fn safe_file_name(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect();
    let s = s.trim().trim_matches('.').to_owned();
    if s.is_empty() { "project".into() } else { s }
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let ws = Arc::new(Mutex::new(Workspace::new(Arc::new(SystemClock))));
    let mut app = PrswtApp::new(ws.clone());
    let server = std::env::var("PRSWT_SERVER")
        .ok()
        .map(|s| s.trim_end_matches('/').to_owned())
        .filter(|s| s.starts_with("https://") || s.starts_with("http://localhost"))
        .unwrap_or_else(|| cloud::DEFAULT_SERVER.to_owned());
    app.cloud.server.clone_from(&server);
    app.today = today();
    if let Some(sample) = flag("--sample") {
        app.open_sample(&sample);
    }
    // `--view risks` etc. opens on that tab (demos, screenshots).
    if let Some(view) = flag("--view")
        && let Some(t) = prswt_ui::Tab::ALL.into_iter().find(|t| {
            let v = view.to_lowercase();
            t.label().to_lowercase().starts_with(&v) || format!("{t:?}").to_lowercase() == v
        })
    {
        app.tab = t;
    }
    if let Some(code) = flag("--select") {
        app.select_activity(&code);
    }
    let theme = match flag("--theme").as_deref() {
        Some("dark") => Some(egui::ThemePreference::Dark),
        Some("light") => Some(egui::ThemePreference::Light),
        _ => None,
    };
    let cloud_open = flag("--cloud-open");
    let shot = flag("--screenshot").map(|p| (PathBuf::from(p), 0, Captured::default(), false));
    if let Some(port) = flag("--control").and_then(|p| p.parse::<u16>().ok()) {
        let shared = ws.clone();
        std::thread::spawn(move || {
            if let Err(e) = prswt_mcp::control::serve(shared, port) {
                eprintln!("control channel: {e}");
            }
        });
        app.status =
            format!("Control channel on 127.0.0.1:{port} (prswt-cli mcp --connect {port})");
    }
    // A file given on the command line (or by the OS file association).
    let mut file = None;
    let mut rest = args.iter().skip(1);
    while let Some(a) = rest.next() {
        if a.starts_with("--") {
            rest.next(); // every flag takes one value
        } else {
            file = Some(PathBuf::from(a));
            break;
        }
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("PRSWT — project controls & risk")
            .with_inner_size([1400.0, 860.0])
            .with_min_inner_size([800.0, 500.0])
            .with_icon(app_icon()),
        persist_window: true,
        ..Default::default()
    };
    let run = eframe::run_native(
        "PRSWT",
        options,
        Box::new(move |cc| {
            prswt_ui::apply_style(&cc.egui_ctx);
            if let Some(t) = theme {
                cc.egui_ctx.set_theme(t);
            }
            app.recent =
                files::parse_recent(cc.storage.and_then(|s| s.get_string(RECENT_KEY)).as_deref());
            // Shown at once; the worker confirms (or signs out) shortly.
            app.cloud.account = cc
                .storage
                .and_then(|s| s.get_string(ACCOUNT_KEY))
                .filter(|e| e.contains('@'));
            let cache = eframe::storage_dir("PRSWT")
                .unwrap_or_else(std::env::temp_dir)
                .join("cloud");
            let client = cloud::Client::start(server.clone(), ws.clone(), cache.clone());
            client.send(cloud::Job::Resume);
            if let Some(id) = &cloud_open {
                client.send(cloud::Job::Open(id.clone()));
                app.cloud.sync.entry(id.clone()).or_default().text = "Syncing…".into();
            }
            let mut w = Window {
                app,
                docs: HashMap::new(),
                pending: None,
                discarded: HashSet::new(),
                closing: false,
                title: String::new(),
                shot,
                cloud: client,
                ws: ws.clone(),
                cache,
                watched: HashMap::new(),
                next_round: SYNC_EVERY_SECS,
                theme_sent: Some(cc.egui_ctx.options(|o| o.theme_preference)),
                revs: HashMap::new(),
                scheduled_at: 0.0,
            };
            if let Some(f) = file {
                w.open_path(&f);
            }
            Ok(Box::new(w))
        }),
    );
    match run {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("prswt: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
