//! SYNC-01..04 in the window (ADR-0010): the account line, the cloud menu,
//! "Open from cloud", sharing, sync state, conflicts and who is here.
//!
//! The UI does no network IO. It shows a [`CloudView`] the desktop shell
//! keeps up to date and queues [`CloudRequest`]s the shell carries out
//! (sign-in through the browser, HTTP calls, background sync).

use std::collections::BTreeMap;

use egui::{Align, Layout, RichText, Ui};
use serde_json::Value;

use crate::PrswtApp;
use crate::theme::{Tokens, semibold};

/// Roles offered when sharing, as stored by the server.
pub const ROLES: [(&str, &str); 3] = [
    ("viewer", "Can view"),
    ("editor", "Can edit"),
    ("owner", "Owner (can share)"),
];

/// Something the shell must do over the network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudRequest {
    /// Sign in through the browser.
    SignIn,
    /// Forget the token on this computer.
    SignOut,
    /// Refresh the list of cloud projects.
    ListProjects,
    /// Upload the open project with this id; it becomes a cloud project.
    Upload(String),
    /// Open the cloud project with this id (cached copy first).
    Open(String),
    /// Sync this project now.
    Sync(String),
    /// Load the members of this project.
    Members(String),
    /// Add a person or change their role: (project, email, role).
    Invite(String, String, String),
    /// Remove a person: (project, email).
    Remove(String, String),
}

/// One project in "Open from cloud".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudProject {
    /// Project id.
    pub id: String,
    /// Project name.
    pub name: String,
    /// The signed-in person's role.
    pub role: String,
}

/// Where a cloud project's local copy stands.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SyncState {
    /// One-line state: "Synced", "Syncing…", "Offline · changes kept" …
    pub text: String,
    /// Branches with changes not yet sent.
    pub pending: u64,
    /// The last sync failed (offline or refused).
    pub problem: bool,
    /// People with the project open now (emails).
    pub present: Vec<String>,
    /// Fields both sides changed, from the last sync (server kept).
    pub conflicts: Vec<Value>,
    /// The latest change someone else made: (who, short text).
    pub last_change: Option<String>,
}

/// Everything the window shows about the cloud.
#[derive(Debug, Clone, Default)]
pub struct CloudView {
    /// Server origin, e.g. `https://prswt.juz.ai`.
    pub server: String,
    /// The signed-in account, if any.
    pub account: Option<String>,
    /// A long action in progress ("Waiting for the browser…").
    pub busy: Option<String>,
    /// Projects the account may open.
    pub projects: Vec<CloudProject>,
    /// Per open cloud project (by id): its sync state.
    pub sync: BTreeMap<String, SyncState>,
    /// Members of the project in the share dialog: (email, role).
    pub members: Vec<(String, String)>,
    /// Last error to show in the cloud dialogs.
    pub error: Option<String>,
}

/// Open cloud dialogs and their inputs.
#[derive(Debug, Clone, Default)]
pub(crate) struct CloudDialogs {
    open: bool,
    share: bool,
    conflicts: bool,
    email: String,
    role: usize,
}

impl PrswtApp {
    /// Requests the shell carries out next (oldest first).
    pub fn take_cloud_requests(&mut self) -> Vec<CloudRequest> {
        std::mem::take(&mut self.cloud_requests)
    }

    /// Whether the selected project is a cloud project.
    pub(crate) fn selected_is_cloud(&mut self) -> Option<String> {
        let (id, _) = self.selected()?;
        self.cloud.sync.contains_key(&id).then_some(id)
    }

    /// File ▸ Cloud.
    pub(crate) fn cloud_menu(&mut self, ui: &mut Ui) {
        ui.menu_button("Cloud", |ui| {
            let signed = self.cloud.account.is_some();
            if !signed {
                if ui.button("Sign in…").clicked() {
                    self.cloud_requests.push(CloudRequest::SignIn);
                }
                ui.label(
                    RichText::new("Sign in to share and sync projects")
                        .small()
                        .weak(),
                );
                return;
            }
            if ui.button("Open from cloud…").clicked() {
                self.dialogs.open = true;
                self.cloud_requests.push(CloudRequest::ListProjects);
            }
            let cloud = self.selected_is_cloud();
            let selected = self.selected().map(|s| s.0);
            match (cloud, selected) {
                (Some(id), _) => {
                    if ui.button("Sync now").clicked() {
                        self.cloud_requests.push(CloudRequest::Sync(id.clone()));
                    }
                    if ui.button("Share…").clicked() {
                        self.open_share(&id);
                    }
                }
                (None, Some(id)) => {
                    if ui
                        .button("Save to cloud")
                        .on_hover_text("Upload this project so it syncs and can be shared")
                        .clicked()
                    {
                        self.cloud_requests.push(CloudRequest::Upload(id));
                    }
                }
                (None, None) => {}
            }
            ui.separator();
            if ui.button("Sign out").clicked() {
                self.cloud_requests.push(CloudRequest::SignOut);
            }
        });
    }

    /// Opens the "Open from cloud" dialog.
    pub(crate) fn show_cloud_open(&mut self) {
        self.dialogs.open = true;
    }

    /// Review requests on the What-if view: sandboxes people or agents
    /// offered for the live plan, with Accept (commit) and Reject (discard).
    pub(crate) fn proposals(&mut self, ui: &mut Ui) {
        let tok = Tokens::for_ui(ui);
        let list = self.cached(|c| &mut c.branches, "sandbox.list", serde_json::json!({}));
        let rows: Vec<Value> = list["branches"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| b["status"] == "open" && b["kind"] == "sandbox")
            .cloned()
            .collect();
        if rows.is_empty() {
            return;
        }
        ui.label(
            RichText::new("WHAT-IF BRANCHES")
                .font(semibold(11.0))
                .color(tok.muted),
        );
        for b in rows {
            let name = b["name"].as_str().unwrap_or("").to_owned();
            ui.horizontal(|ui| {
                ui.label(RichText::new(&name).font(semibold(13.5)));
                let by = author_name(&b["created_by"]);
                ui.label(RichText::new(format!("by {by}")).small().color(tok.muted));
                if !b["proposal"].is_null() {
                    let who = author_name(&b["proposal"]["by"]);
                    ui.label(
                        RichText::new(format!("Proposed by {who}"))
                            .small()
                            .color(tok.critical),
                    );
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .button("Reject")
                        .on_hover_text("Discard this branch")
                        .clicked()
                        && self
                            .exec("sandbox.discard", serde_json::json!({ "sandbox": name }))
                            .is_some()
                        && self.branch == name
                    {
                        self.branch = "live".into();
                    }
                    if ui
                        .button(RichText::new("Accept").color(tok.ok))
                        .on_hover_text("Commit this branch to the live plan")
                        .clicked()
                        && self
                            .exec("sandbox.commit", serde_json::json!({ "sandbox": name }))
                            .is_some()
                        && self.branch == name
                    {
                        self.branch = "live".into();
                    }
                    if b["proposal"].is_null() {
                        if ui
                            .button("Propose…")
                            .on_hover_text("Ask an owner to review this branch")
                            .clicked()
                        {
                            self.open_command("sandbox.propose", &[("sandbox", &name)]);
                        }
                    } else if ui.button("Withdraw").clicked() {
                        self.exec(
                            "sandbox.propose",
                            serde_json::json!({ "sandbox": name, "withdraw": true }),
                        );
                    }
                    if self.branch != name && ui.button("Open").clicked() {
                        self.branch.clone_from(&name);
                        self.refresh();
                    }
                });
            });
            if let Some(note) = b["proposal"]["note"].as_str().filter(|n| !n.is_empty()) {
                ui.label(RichText::new(format!("“{note}”")).color(tok.muted));
            }
            ui.separator();
        }
        ui.add_space(6.0);
    }

    fn open_share(&mut self, id: &str) {
        self.dialogs.share = true;
        self.dialogs.email.clear();
        self.cloud.members.clear();
        self.cloud_requests
            .push(CloudRequest::Members(id.to_owned()));
    }

    /// The account block at the bottom of the navigation panel.
    pub(crate) fn account_block(&mut self, ui: &mut Ui, tok: &Tokens) {
        if let Some(busy) = &self.cloud.busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(busy).small().color(tok.muted));
            });
            return;
        }
        match self.cloud.account.clone() {
            None => {
                if ui
                    .add(
                        egui::Button::new("Sign in to sync")
                            .min_size(egui::Vec2::new(ui.available_width(), 30.0)),
                    )
                    .on_hover_text(
                        "Keep projects on the server, use them on several computers and share them",
                    )
                    .clicked()
                {
                    self.cloud_requests.push(CloudRequest::SignIn);
                }
            }
            Some(email) => {
                if let Some(id) = self.selected_is_cloud() {
                    let st = self.cloud.sync.get(&id).cloned().unwrap_or_default();
                    let colour = if st.problem || !st.conflicts.is_empty() {
                        tok.critical
                    } else {
                        tok.ok
                    };
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(&st.text).small().color(colour));
                        if !st.conflicts.is_empty()
                            && ui
                                .small_button(format!("{} conflict(s)", st.conflicts.len()))
                                .clicked()
                        {
                            self.dialogs.conflicts = true;
                        }
                    });
                    if let Some(last) = &st.last_change {
                        ui.label(RichText::new(last).small().color(tok.muted));
                    }
                }
                ui.label(RichText::new(email).small().color(tok.muted));
            }
        }
    }

    /// Faces of the people who have the selected cloud project open, for
    /// the header.
    pub(crate) fn presence(&mut self, ui: &mut Ui, tok: &Tokens) {
        let Some(id) = self.selected_is_cloud() else {
            return;
        };
        let st = self.cloud.sync.get(&id).cloned().unwrap_or_default();
        let others: Vec<&String> = st
            .present
            .iter()
            .filter(|e| Some(*e) != self.cloud.account.as_ref())
            .collect();
        for (k, email) in others.iter().take(4).enumerate() {
            let (rect, resp) =
                ui.allocate_exact_size(egui::Vec2::splat(24.0), egui::Sense::hover());
            let palette = [tok.accent, tok.critical, tok.ok, tok.normal];
            let fill = palette
                .get(k % palette.len())
                .copied()
                .unwrap_or(tok.accent);
            ui.painter().circle_filled(rect.center(), 12.0, fill);
            let initial: String = email
                .chars()
                .next()
                .map(|c| c.to_uppercase().collect())
                .unwrap_or_default();
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                initial,
                semibold(12.0),
                egui::Color32::WHITE,
            );
            resp.on_hover_text(format!("{email} has this project open"));
        }
        if ui
            .add(egui::Button::new("Share").min_size(egui::Vec2::new(0.0, 32.0)))
            .on_hover_text("Invite people and set their roles")
            .clicked()
        {
            self.open_share(&id);
        }
    }

    /// The cloud dialogs: open, share, conflicts.
    pub(crate) fn cloud_dialogs(&mut self, ui: &mut Ui) {
        let tok = Tokens::for_ui(ui);
        if self.dialogs.open {
            let mut open = None;
            let r = egui::Modal::new(egui::Id::unique("cloud_open")).show(ui.ctx(), |ui| {
                ui.set_width(520.0);
                ui.heading("Open from cloud");
                ui.label(RichText::new("Projects shared with you on the server.").color(tok.muted));
                ui.add_space(8.0);
                if let Some(e) = &self.cloud.error {
                    ui.colored_label(tok.critical, e);
                }
                if self.cloud.projects.is_empty() {
                    ui.label(RichText::new("No cloud projects yet. Open a project and use File ▸ Cloud ▸ Save to cloud.").color(tok.muted));
                }
                egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                    for p in &self.cloud.projects {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&p.name).font(semibold(13.5)));
                            ui.label(RichText::new(role_label(&p.role)).small().color(tok.muted));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("Open").clicked() {
                                    open = Some(p.id.clone());
                                }
                            });
                        });
                        ui.separator();
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button("Refresh").clicked() {
                        self.cloud_requests.push(CloudRequest::ListProjects);
                    }
                    ui.button("Close").clicked()
                })
                .inner
            });
            if let Some(id) = open {
                self.cloud_requests.push(CloudRequest::Open(id));
                self.dialogs.open = false;
            } else if r.inner || r.should_close() {
                self.dialogs.open = false;
            }
        }
        if self.dialogs.share
            && let Some(id) = self.selected_is_cloud()
        {
            let me = self.cloud.account.clone().unwrap_or_default();
            let owner = self
                .cloud
                .members
                .iter()
                .any(|(e, r)| *e == me && r == "owner");
            let present = self
                .cloud
                .sync
                .get(&id)
                .map(|s| s.present.clone())
                .unwrap_or_default();
            let mut close = false;
            let r = egui::Modal::new(egui::Id::unique("cloud_share")).show(ui.ctx(), |ui| {
                ui.set_width(540.0);
                ui.heading("Share project");
                ui.label(
                    RichText::new("People you add can sign in with their Microsoft work account.")
                        .color(tok.muted),
                );
                ui.add_space(8.0);
                if let Some(e) = &self.cloud.error {
                    ui.colored_label(tok.critical, e);
                }
                for (email, role) in self.cloud.members.clone() {
                    ui.horizontal(|ui| {
                        let here = if present.contains(&email) {
                            " · here now"
                        } else {
                            ""
                        };
                        ui.label(format!("{email}{here}"));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if owner && email != me && ui.small_button("Remove").clicked() {
                                self.cloud_requests
                                    .push(CloudRequest::Remove(id.clone(), email.clone()));
                            }
                            let mut pick = ROLES.iter().position(|r| r.0 == role).unwrap_or(0);
                            ui.add_enabled_ui(owner && email != me, |ui| {
                                egui::ComboBox::from_id_salt(format!("role-{email}"))
                                    .selected_text(ROLES[pick].1)
                                    .show_ui(ui, |ui| {
                                        for (k, r) in ROLES.iter().enumerate() {
                                            ui.selectable_value(&mut pick, k, r.1);
                                        }
                                    });
                            });
                            if ROLES[pick].0 != role {
                                self.cloud_requests.push(CloudRequest::Invite(
                                    id.clone(),
                                    email.clone(),
                                    ROLES[pick].0.to_owned(),
                                ));
                            }
                        });
                    });
                }
                if owner {
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.dialogs.email)
                                .hint_text("name@company.com")
                                .desired_width(260.0),
                        );
                        egui::ComboBox::from_id_salt("invite-role")
                            .selected_text(ROLES[self.dialogs.role.min(2)].1)
                            .show_ui(ui, |ui| {
                                for (k, r) in ROLES.iter().enumerate() {
                                    ui.selectable_value(&mut self.dialogs.role, k, r.1);
                                }
                            });
                        let email = self.dialogs.email.trim().to_owned();
                        if ui
                            .add_enabled(email.contains('@'), egui::Button::new("Invite"))
                            .clicked()
                        {
                            self.cloud_requests.push(CloudRequest::Invite(
                                id.clone(),
                                email,
                                ROLES[self.dialogs.role.min(2)].0.to_owned(),
                            ));
                            self.dialogs.email.clear();
                        }
                    });
                } else {
                    ui.label(
                        RichText::new("Only owners can add people or change roles.")
                            .small()
                            .color(tok.muted),
                    );
                }
                ui.add_space(6.0);
                close = ui.button("Close").clicked();
            });
            if close || r.should_close() {
                self.dialogs.share = false;
            }
        }
        if self.dialogs.conflicts {
            let id = self.selected_is_cloud().unwrap_or_default();
            let list = self
                .cloud
                .sync
                .get(&id)
                .map(|s| s.conflicts.clone())
                .unwrap_or_default();
            let r = egui::Modal::new(egui::Id::unique("cloud_conflicts")).show(ui.ctx(), |ui| {
                ui.set_width(560.0);
                ui.heading("Changes made by two people");
                ui.label(RichText::new("Someone changed the same thing on the server while you were working. Their version was kept; yours is shown so you can redo it if needed.").color(tok.muted));
                ui.add_space(6.0);
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    for c in &list {
                        ui.label(conflict_text(c));
                        ui.separator();
                    }
                });
                ui.button("OK").clicked()
            });
            if r.inner || r.should_close() {
                self.dialogs.conflicts = false;
                if let Some(st) = self.cloud.sync.get_mut(&id) {
                    st.conflicts.clear();
                }
            }
        }
    }
}

/// A person's or agent's name from an `Author` value.
fn author_name(v: &Value) -> String {
    match v["type"].as_str() {
        Some("agent") => format!("agent {}", v["name"].as_str().unwrap_or("")),
        Some("system") => "PRSWT".to_owned(),
        _ => v["name"].as_str().unwrap_or("someone").to_owned(),
    }
}

/// `editor` → `Can edit`.
pub fn role_label(role: &str) -> &'static str {
    ROLES.iter().find(|r| r.0 == role).map_or("Member", |r| r.1)
}

/// One conflict in words: what, on which branch, kept and yours.
fn conflict_text(c: &Value) -> String {
    let kind = c["entity"]["kind"].as_str().unwrap_or("item");
    let field = c["field"].as_str().unwrap_or("*");
    let show = |v: &Value| match v {
        Value::Null => "removed".to_owned(),
        Value::String(s) => format!("“{s}”"),
        other => {
            let t = other.to_string();
            if t.len() > 60 {
                format!(
                    "{}…",
                    &t[..t.char_indices().nth(57).map_or(t.len(), |x| x.0)]
                )
            } else {
                t
            }
        }
    };
    let what = if field == "*" {
        format!("{kind} (whole item)")
    } else {
        format!("{kind} · {field}")
    };
    format!(
        "{what} on {}: kept {} · yours was {}",
        c["branch"].as_str().unwrap_or("live"),
        show(&c["server"]),
        show(&c["yours"])
    )
}
