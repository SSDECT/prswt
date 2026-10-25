//! DSK-07: the window chrome of design "B · Modern workspace": the
//! navigation panel, the header (breadcrumb, branch, command search,
//! primary actions), the summary cards, the activity inspector, the
//! command palette (Ctrl+K) and the status bar.
//!
//! Everything here only arranges what the views already offer; every
//! action still ends in a catalog command or a [`FileRequest`].

use std::fmt::Write as _;

use egui::{
    Align, Align2, Button, FontId, Frame, Key, KeyboardShortcut, Layout, Margin, Modifiers, Pos2,
    Rect, RichText, ScrollArea, Sense, Stroke, Ui, Vec2,
};
use serde_json::{Value, json};

use crate::theme::{Tokens, semibold};
use crate::{
    Cache, FileRequest, Format, NewProject, PrswtApp, SAMPLES, Tab, clip, day_date, forms,
    longest_path_float, on_longest_path,
};

/// Width of the navigation panel in points.
const NAV_W: f32 = 232.0;
/// Width of the activity inspector in points.
const INSPECTOR_W: f32 = 320.0;
/// Most entries the command palette lists at once.
const PALETTE_ROWS: usize = 12;

/// What a command-palette entry does.
#[derive(Debug, Clone)]
pub(crate) enum Action {
    /// Open a command dialog.
    Form(&'static forms::FormSpec),
    /// Switch to a view.
    View(Tab),
    /// Hand a file action to the shell.
    File(FileRequest),
    /// File ▸ New project.
    NewProject,
    /// Open a built-in sample.
    Sample(&'static str),
    /// Undo or redo (`edit.undo` / `edit.redo`).
    Edit(&'static str),
    /// A cloud action.
    Cloud(crate::CloudRequest),
}

/// The open command palette.
#[derive(Debug, Default)]
pub(crate) struct Palette {
    query: String,
    selected: usize,
}

/// A white card: thin border, rounded corners, inner padding.
pub(crate) fn card(tok: &Tokens) -> Frame {
    Frame::new()
        .fill(tok.surface)
        .stroke(Stroke::new(1.0, tok.line))
        .corner_radius(10)
        .inner_margin(Margin::same(12))
}

/// The accent-filled main button of a screen.
pub(crate) fn primary(ui: &mut Ui, tok: &Tokens, text: &str) -> egui::Response {
    ui.add(
        Button::new(
            RichText::new(text)
                .font(semibold(13.5))
                .color(egui::Color32::WHITE),
        )
        .fill(tok.accent)
        .stroke(Stroke::NONE)
        .corner_radius(8)
        .min_size(Vec2::new(0.0, 32.0)),
    )
}

/// A row of mutually exclusive choices; returns the index clicked.
pub(crate) fn segmented(
    ui: &mut Ui,
    tok: &Tokens,
    options: &[&str],
    selected: usize,
) -> Option<usize> {
    let mut hit = None;
    Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .stroke(Stroke::new(1.0, tok.line))
        .corner_radius(8)
        .inner_margin(Margin::same(2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal(|ui| {
                for (k, label) in options.iter().enumerate() {
                    let on = k == selected;
                    let text = if on {
                        RichText::new(*label).font(semibold(12.5)).color(tok.ink)
                    } else {
                        RichText::new(*label).size(12.5).color(tok.muted)
                    };
                    let b = Button::new(text)
                        .corner_radius(6)
                        .min_size(Vec2::new(0.0, 24.0));
                    let b = if on {
                        b.fill(tok.surface).stroke(Stroke::new(1.0, tok.line))
                    } else {
                        b.frame(false)
                    };
                    if ui.add(b).clicked() {
                        hit = Some(k);
                    }
                }
            });
        });
    hit
}

impl PrswtApp {
    /// The left navigation panel: logo, project switcher, the views in two
    /// groups (Plan, Analyse) and the save state at the bottom.
    pub(crate) fn sidebar(&mut self, ui: &mut Ui) {
        let tok = Tokens::for_ui(ui);
        egui::Panel::left("nav")
            .exact_size(NAV_W)
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(tok.sidebar)
                    .inner_margin(Margin::symmetric(10, 14)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    logo(ui, &tok);
                    ui.label(RichText::new("PRSWT").font(semibold(15.0)).color(tok.ink));
                });
                ui.add_space(12.0);
                self.project_switcher(ui, &tok);
                ui.add_space(10.0);
                section(ui, &tok, "PLAN");
                for t in [Tab::Gantt, Tab::Activities, Tab::Resources] {
                    self.nav_item(ui, &tok, t);
                }
                ui.add_space(8.0);
                section(ui, &tok, "ANALYSE");
                for t in [Tab::Risks, Tab::Simulation, Tab::Sandboxes] {
                    self.nav_item(ui, &tok, t);
                }
                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    self.account_block(ui, &tok);
                    if self.project.is_some() && self.selected_is_cloud().is_none() {
                        let (text, colour) = if self.unsaved {
                            ("Unsaved changes · Ctrl+S saves", tok.critical)
                        } else {
                            ("Saved · all changes on disk", tok.muted)
                        };
                        ui.label(RichText::new(text).small().color(colour));
                    }
                    ui.separator();
                });
            });
    }

    /// One view entry of the navigation panel.
    fn nav_item(&mut self, ui: &mut Ui, tok: &Tokens, tab: Tab) {
        let enabled = self.project.is_some();
        let active = enabled && self.tab == tab;
        let sense = if enabled {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), sense);
        let p = ui.painter();
        if active {
            p.rect_filled(rect, 7.0, tok.accent_soft);
        } else if enabled && resp.hovered() {
            p.rect_filled(rect, 7.0, ui.visuals().faint_bg_color);
        }
        let mid = rect.center().y;
        p.rect_filled(
            Rect::from_center_size(Pos2::new(rect.min.x + 14.0, mid), Vec2::splat(8.0)),
            2.0,
            if active { tok.accent } else { tok.summary },
        );
        let (font, colour) = match (enabled, active) {
            (false, _) => (FontId::proportional(13.5), tok.float),
            (true, true) => (semibold(13.5), tok.ink),
            (true, false) => (FontId::proportional(13.5), tok.ink),
        };
        p.text(
            Pos2::new(rect.min.x + 28.0, mid),
            Align2::LEFT_CENTER,
            tab.label(),
            font,
            colour,
        );
        let key = ui
            .ctx()
            .format_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, tab.key()));
        p.text(
            Pos2::new(rect.max.x - 8.0, mid),
            Align2::RIGHT_CENTER,
            key,
            FontId::proportional(11.0),
            tok.muted,
        );
        if resp.clicked() {
            self.tab = tab;
        }
    }

    /// The project card at the top of the navigation panel; clicking it
    /// lists the open projects and the ways to open another.
    fn project_switcher(&mut self, ui: &mut Ui, tok: &Tokens) {
        let w = ui.available_width();
        let (title, sub) = match &self.project {
            Some(p) => (p.clone(), format!("Branch: {}", self.branch)),
            None => (
                "No project open".to_owned(),
                "New, open or sample".to_owned(),
            ),
        };
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 46.0), Sense::click());
        let p = ui.painter();
        p.rect_filled(rect, 8.0, tok.surface);
        p.rect_stroke(
            rect,
            8.0,
            Stroke::new(1.0, if resp.hovered() { tok.float } else { tok.line }),
            egui::StrokeKind::Inside,
        );
        p.text(
            Pos2::new(rect.min.x + 10.0, rect.min.y + 15.0),
            Align2::LEFT_CENTER,
            clip(&title, w - 40.0, 7.4),
            semibold(13.0),
            tok.ink,
        );
        p.text(
            Pos2::new(rect.min.x + 10.0, rect.min.y + 32.0),
            Align2::LEFT_CENTER,
            clip(&sub, w - 40.0, 6.4),
            FontId::proportional(11.0),
            tok.muted,
        );
        chevron(p, Pos2::new(rect.max.x - 16.0, rect.center().y), tok.muted);
        egui::Popup::menu(&resp).width(w).show(|ui| {
            for n in self.project_names() {
                if ui
                    .selectable_label(self.project.as_ref() == Some(&n), &n)
                    .clicked()
                {
                    self.switch_project(n);
                }
            }
            if self.project.is_some() {
                ui.separator();
            }
            if ui.button("New project…").clicked() {
                self.new_project = Some(NewProject::starting(self.today));
            }
            if ui.button("Open…").clicked() {
                self.requests.push(FileRequest::Open);
            }
            ui.menu_button("Open sample", |ui| {
                for s in SAMPLES {
                    if ui.button(s).clicked() {
                        self.open_sample(s);
                    }
                }
            });
        });
    }

    /// Selects another open project (its live branch).
    pub(crate) fn switch_project(&mut self, name: String) {
        if self.project.as_ref() != Some(&name) {
            self.project = Some(name);
            self.branch = "live".into();
            self.edit_activity.clear();
            self.cache = Cache::default();
        }
    }

    /// The header above the views: where you are, which branch, the command
    /// search and the two most used actions.
    pub(crate) fn header(&mut self, ui: &mut Ui) {
        let tok = Tokens::for_ui(ui);
        egui::Panel::top("header")
            .frame(
                Frame::new()
                    .fill(tok.surface)
                    .inner_margin(Margin::symmetric(20, 10)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let project = self.project.clone().unwrap_or_default();
                    ui.label(RichText::new(project).size(14.0).color(tok.muted));
                    ui.label(RichText::new("/").size(14.0).color(tok.float));
                    ui.label(
                        RichText::new(self.tab.label())
                            .font(semibold(14.0))
                            .color(tok.ink),
                    );
                    ui.add_space(4.0);
                    self.branch_pill(ui, &tok);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if primary(ui, &tok, "+  Activity").clicked() {
                            self.open_command("activity.add", &[]);
                        }
                        if ui
                            .add(Button::new("Risk analysis").min_size(Vec2::new(0.0, 32.0)))
                            .on_hover_text("Run a Monte Carlo analysis of the selected branch")
                            .clicked()
                        {
                            self.tab = Tab::Simulation;
                            let p = self.with_branch(json!({ "iterations": self.iterations }));
                            self.exec_long("simulation.run", p);
                        }
                        if search_box(ui, &tok).clicked() {
                            self.palette = Some(Palette::default());
                        }
                        self.presence(ui, &tok);
                    });
                });
            });
    }

    /// The branch badge: `Live` or the sandbox name; lists the open branches.
    fn branch_pill(&mut self, ui: &mut Ui, tok: &Tokens) {
        let live = self.branch == "live";
        let text = if live {
            "Live".to_owned()
        } else {
            format!("What-if: {}", self.branch)
        };
        let (fill, ink) = if live {
            (tok.accent_soft, tok.accent)
        } else {
            (tok.critical.gamma_multiply(0.18), tok.critical)
        };
        let galley = ui
            .painter()
            .layout_no_wrap(text, FontId::proportional(12.0), ink);
        let size = Vec2::new(galley.size().x + 28.0, 22.0);
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        let p = ui.painter();
        p.rect_filled(rect, 11.0, fill);
        p.circle_filled(Pos2::new(rect.min.x + 11.0, rect.center().y), 3.0, ink);
        p.galley(
            Pos2::new(rect.min.x + 19.0, rect.center().y - galley.size().y / 2.0),
            galley,
            ink,
        );
        let resp = resp.on_hover_text("Branch: the live plan or a what-if sandbox");
        egui::Popup::menu(&resp).show(|ui| {
            let branches = self.cached(|c| &mut c.branches, "sandbox.list", json!({}));
            let names: Vec<String> = branches["branches"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|b| b["status"] == "open")
                .filter_map(|b| b["name"].as_str().map(str::to_owned))
                .collect();
            for n in names {
                if ui.selectable_label(self.branch == n, &n).clicked() && self.branch != n {
                    self.branch = n;
                    self.edit_activity.clear();
                    self.cache = Cache::default();
                }
            }
            ui.separator();
            if ui.button("Manage what-if branches…").clicked() {
                self.tab = Tab::Sandboxes;
            }
        });
    }

    /// Four summary cards: finish, deadline and float, P80 and activities.
    pub(crate) fn kpis(&mut self, ui: &mut Ui) {
        let tok = Tokens::for_ui(ui);
        let s = self.cached(|c| &mut c.summary, "schedule.run", json!({}));
        let rows = self.rows();
        let path_float = longest_path_float(&rows);
        let on_path = rows
            .iter()
            .filter(|r| on_longest_path(r, path_float))
            .count();
        let sim = self.simulation.clone();
        let mut run = false;
        let mut deadline = false;
        ui.columns(4, |cols| {
            kpi(&mut cols[0], &tok, "Finish", |ui| {
                big(ui, &tok, &day_date(&s["finish"]));
            });
            kpi(&mut cols[1], &tok, "Must finish by", |ui| {
                if s["must_finish_by"].is_null() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Not set").size(14.0).color(tok.muted));
                        deadline = ui.link("Set…").clicked();
                    });
                } else {
                    ui.horizontal(|ui| {
                        big(ui, &tok, &day_date(&s["must_finish_by"]));
                        if let Some(f) = path_float {
                            let (t, c) = if f >= 0.0 {
                                (format!("{f:.0}d float"), tok.ok)
                            } else {
                                (format!("{:.0}d late", -f), tok.critical)
                            };
                            ui.label(RichText::new(t).font(semibold(12.5)).color(c));
                        }
                    });
                }
            });
            kpi(
                &mut cols[2],
                &tok,
                "P80 finish (risk analysis)",
                |ui| match &sim {
                    Some(sim) => {
                        ui.horizontal(|ui| {
                            big(ui, &tok, &day_date(&sim["finish"]["p80"]));
                            ui.label(
                                RichText::new(format!("P50 {}", day_date(&sim["finish"]["p50"])))
                                    .small()
                                    .color(tok.muted),
                            );
                        });
                    }
                    None => {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Not run yet ·").size(14.0).color(tok.muted));
                            run = ui.link("Run").clicked();
                        });
                    }
                },
            );
            kpi(&mut cols[3], &tok, "Activities", |ui| {
                ui.horizontal(|ui| {
                    big(ui, &tok, &rows.len().to_string());
                    if on_path > 0 {
                        ui.label(
                            RichText::new(format!("{on_path} on the longest path"))
                                .font(semibold(12.5))
                                .color(tok.critical),
                        );
                    }
                });
            });
        });
        if run {
            self.tab = Tab::Simulation;
            let p = self.with_branch(json!({ "iterations": self.iterations }));
            self.exec_long("simulation.run", p);
        }
        if deadline {
            self.open_command("project.update", &[]);
        }
    }

    /// The right-hand panel for the selected activity: dates, duration
    /// range, links, why it starts when it does, risks and actions.
    pub(crate) fn inspector(&mut self, ui: &mut Ui) {
        if !matches!(self.tab, Tab::Gantt | Tab::Activities) || self.edit_activity.is_empty() {
            return;
        }
        let code = self.edit_activity.clone();
        let a = self.activity_details(&code);
        if a.is_null() {
            self.edit_activity.clear();
            return;
        }
        let lookups = self.lookups();
        let name_of = |list: &str, code: &str| {
            lookups[list]
                .as_array()
                .into_iter()
                .flatten()
                .find(|x| x["code"] == code)
                .and_then(|x| x["name"].as_str())
                .unwrap_or("")
                .to_owned()
        };
        let rows = self.rows();
        let path_float = longest_path_float(&rows);
        let tok = Tokens::for_ui(ui);
        let mut close = false;
        let mut open: Option<(&'static str, &'static str)> = None;
        egui::Panel::right("inspector")
            .exact_size(INSPECTOR_W)
            .resizable(false)
            .frame(
                Frame::new()
                    .fill(tok.surface)
                    .inner_margin(Margin::same(16)),
            )
            .show(ui, |ui| {
                egui::Panel::bottom("insp_actions_bar")
                    .frame(
                        Frame::new()
                            .fill(tok.surface)
                            .inner_margin(Margin::symmetric(0, 10)),
                    )
                    .show(ui, |ui| {
                        egui::Grid::new("insp_actions")
                            .num_columns(2)
                            .spacing([8.0, 8.0])
                            .show(ui, |ui| {
                                let wide = Vec2::new((INSPECTOR_W - 32.0 - 8.0) / 2.0 - 2.0, 32.0);
                                for (k, (label, command, key)) in [
                                    ("Update progress", "activity.progress", "activity"),
                                    ("Edit", "activity.update", "activity"),
                                    ("Add successor", "relationship.add", "pred"),
                                    ("Assign resource", "resource.assign", "activity"),
                                    ("Add expense", "expense.add", "activity"),
                                    ("Delete", "activity.delete", "activity"),
                                ]
                                .into_iter()
                                .enumerate()
                                {
                                    let text = if command == "activity.delete" {
                                        RichText::new(label).color(tok.danger)
                                    } else {
                                        RichText::new(label)
                                    };
                                    if ui.add(Button::new(text).min_size(wide)).clicked() {
                                        open = Some((command, key));
                                    }
                                    if k % 2 == 1 {
                                        ui.end_row();
                                    }
                                }
                            });
                    });
                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let wbs = a["wbs"].as_str().unwrap_or("");
                        ui.horizontal(|ui| {
                            let place = if wbs.is_empty() {
                                code.clone()
                            } else {
                                format!("{code} · {}", name_of("wbs", wbs))
                            };
                            ui.label(RichText::new(place).small().color(tok.muted));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                close = ui
                                    .add(Button::new("×").frame(false))
                                    .on_hover_text("Close")
                                    .clicked();
                            });
                        });
                        ui.label(
                            RichText::new(a["name"].as_str().unwrap_or(""))
                                .font(semibold(17.0))
                                .color(tok.ink),
                        );
                        ui.horizontal_wrapped(|ui| {
                            if a["critical"] == true {
                                pill(ui, "Critical", tok.critical);
                            } else if on_longest_path(&a, path_float) {
                                pill(ui, "On the longest path", tok.critical);
                            }
                            pill(ui, &status_text(&a["status"]), tok.muted);
                        });
                        ui.add_space(6.0);
                        egui::Grid::new("insp_dates")
                            .num_columns(2)
                            .min_col_width((INSPECTOR_W - 56.0) / 2.0)
                            .spacing([24.0, 8.0])
                            .show(ui, |ui| {
                                fact(ui, &tok, "Start", &day_date(&a["early_start"]));
                                fact(ui, &tok, "Finish", &day_date(&a["early_finish"]));
                                ui.end_row();
                                fact(
                                    ui,
                                    &tok,
                                    "Total float",
                                    a["total_float"].as_str().unwrap_or("–"),
                                );
                                fact(
                                    ui,
                                    &tok,
                                    "Free float",
                                    a["free_float"].as_str().unwrap_or("–"),
                                );
                                ui.end_row();
                                fact(
                                    ui,
                                    &tok,
                                    "Remaining",
                                    a["remaining"].as_str().unwrap_or("–"),
                                );
                                fact(ui, &tok, "Late finish", &day_date(&a["late_finish"]));
                                ui.end_row();
                            });
                        block(ui, &tok, "Duration");
                        duration_range(ui, &tok, &a["duration"]);
                        block(ui, &tok, "Links");
                        let mut any = false;
                        for (key, word) in [("predecessors", "After"), ("successors", "Before")] {
                            for l in a[key].as_array().into_iter().flatten() {
                                any = true;
                                let other = l["activity"].as_str().unwrap_or("");
                                let lag = l["lag"].as_str().unwrap_or("0d");
                                let kind = l["type"].as_str().unwrap_or("FS");
                                ui.horizontal(|ui| {
                                    ui.label(format!(
                                        "{word} {other} {}",
                                        name_of("activities", other)
                                    ));
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        let t = if lag == "0d" {
                                            kind.to_owned()
                                        } else {
                                            format!("{kind} +{lag}")
                                        };
                                        ui.label(RichText::new(t).small().color(tok.muted));
                                    });
                                });
                            }
                        }
                        if !any {
                            ui.label(RichText::new("No links yet.").color(tok.muted));
                        }
                        block(ui, &tok, "Why these dates?");
                        ui.label(RichText::new(explain(&a)).color(tok.ink));
                        let risks = a["risks"].as_array().cloned().unwrap_or_default();
                        if !risks.is_empty() {
                            block(ui, &tok, "Risks");
                            for r in &risks {
                                let p = r["probability"]
                                    .as_str()
                                    .and_then(|s| s.parse::<f64>().ok())
                                    .or_else(|| r["probability"].as_f64())
                                    .unwrap_or(0.0);
                                ui.label(format!(
                                    "{} {} · {:.0} %",
                                    r["code"].as_str().unwrap_or(""),
                                    r["title"].as_str().unwrap_or(""),
                                    p * 100.0
                                ));
                            }
                        }
                        let res = a["assignments"].as_array().cloned().unwrap_or_default();
                        if !res.is_empty() {
                            block(ui, &tok, "Resources");
                            for r in &res {
                                let who = r["resource"]
                                    .as_str()
                                    .or_else(|| r["code"].as_str())
                                    .unwrap_or("");
                                ui.label(format!("{who} {}", name_of("resources", who)));
                            }
                        }
                    });
            });
        if close {
            self.edit_activity.clear();
        }
        if let Some((command, key)) = open {
            self.open_command(command, &[(key, &code)]);
        }
    }

    /// `schedule.activity` for `code`, cached until something changes.
    fn activity_details(&mut self, code: &str) -> Value {
        if let Some((c, v)) = &self.cache.activity
            && c == code
        {
            return v.clone();
        }
        let p = self.with_branch(json!({ "activity": code }));
        let v = self.exec("schedule.activity", p).unwrap_or(Value::Null);
        self.cache.activity = Some((code.to_owned(), v.clone()));
        v
    }

    /// Every palette entry that applies now: (label, group, action).
    fn palette_entries(&self) -> Vec<(String, &'static str, Action)> {
        let open = self.project.is_some();
        let mut out: Vec<(String, &'static str, Action)> = vec![
            ("New project…".into(), "File", Action::NewProject),
            ("Open…".into(), "File", Action::File(FileRequest::Open)),
        ];
        for s in SAMPLES {
            out.push((format!("Open sample: {s}"), "File", Action::Sample(s)));
        }
        if self.cloud.account.is_none() {
            out.push((
                "Sign in to sync".into(),
                "Cloud",
                Action::Cloud(crate::CloudRequest::SignIn),
            ));
        } else {
            out.push((
                "Open from cloud…".into(),
                "Cloud",
                Action::Cloud(crate::CloudRequest::ListProjects),
            ));
        }
        for f in Format::ALL {
            out.push((
                format!("Import {}…", f.label()),
                "File",
                Action::File(FileRequest::Import(f)),
            ));
        }
        if open {
            out.push(("Save".into(), "File", Action::File(FileRequest::Save)));
            out.push(("Save as…".into(), "File", Action::File(FileRequest::SaveAs)));
            for f in Format::ALL {
                out.push((
                    format!("Export {}…", f.label()),
                    "File",
                    Action::File(FileRequest::Export(f)),
                ));
            }
            out.push((
                "Close project".into(),
                "File",
                Action::File(FileRequest::Close),
            ));
            out.push(("Undo".into(), "Edit", Action::Edit("edit.undo")));
            out.push(("Redo".into(), "Edit", Action::Edit("edit.redo")));
            for t in Tab::ALL {
                out.push((format!("Go to {}", t.label()), "View", Action::View(t)));
            }
        }
        for spec in forms::FORMS {
            if open || spec.command == "commands.list" {
                out.push((spec.title.to_owned(), spec.menu, Action::Form(spec)));
            }
        }
        out
    }

    /// The command palette: type a few letters of any command, view or
    /// file action; arrows choose, Enter runs, Esc closes.
    pub(crate) fn palette_ui(&mut self, ui: &mut Ui) {
        let Some(mut pal) = self.palette.take() else {
            return;
        };
        let tok = Tokens::for_ui(ui);
        let entries = self.palette_entries();
        let query = pal.query.to_lowercase();
        let words: Vec<&str> = query.split_whitespace().collect();
        let hits: Vec<&(String, &'static str, Action)> = entries
            .iter()
            .filter(|e| {
                let text = format!("{} {}", e.0, e.1).to_lowercase();
                words.iter().all(|w| text.contains(w))
            })
            .take(PALETTE_ROWS)
            .collect();
        let (up, down, enter) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::Enter),
            )
        });
        if up {
            pal.selected = pal.selected.saturating_sub(1);
        }
        if down {
            pal.selected = pal.selected.saturating_add(1);
        }
        pal.selected = pal.selected.min(hits.len().saturating_sub(1));
        let mut chosen = enter
            .then(|| hits.get(pal.selected).map(|e| e.2.clone()))
            .flatten();
        let r = egui::Modal::new(egui::Id::unique("palette")).show(ui.ctx(), |ui| {
            ui.set_width(560.0);
            let edit = ui.add(
                egui::TextEdit::singleline(&mut pal.query)
                    .hint_text("Search commands, views and files")
                    .desired_width(f32::INFINITY)
                    .font(FontId::proportional(15.0))
                    .margin(Margin::symmetric(10, 8)),
            );
            edit.request_focus();
            ui.add_space(6.0);
            if hits.is_empty() {
                ui.label(RichText::new("Nothing matches.").color(tok.muted));
            }
            for (k, (label, group, action)) in hits.iter().map(|e| (&e.0, e.1, &e.2)).enumerate() {
                let (rect, resp) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::click());
                if k == pal.selected || resp.hovered() {
                    ui.painter().rect_filled(rect, 6.0, tok.accent_soft);
                }
                ui.painter().text(
                    Pos2::new(rect.min.x + 10.0, rect.center().y),
                    Align2::LEFT_CENTER,
                    label,
                    FontId::proportional(13.5),
                    tok.ink,
                );
                ui.painter().text(
                    Pos2::new(rect.max.x - 10.0, rect.center().y),
                    Align2::RIGHT_CENTER,
                    group,
                    FontId::proportional(11.5),
                    tok.muted,
                );
                if resp.clicked() {
                    chosen = Some(action.clone());
                }
            }
        });
        match chosen {
            Some(a) => self.run_action(a),
            None if r.should_close() => {}
            None => self.palette = Some(pal),
        }
    }

    /// Carries out a palette entry.
    fn run_action(&mut self, a: Action) {
        match a {
            Action::Form(spec) => self.open_form(spec),
            Action::View(t) => self.tab = t,
            Action::File(r) => self.requests.push(r),
            Action::NewProject => self.new_project = Some(NewProject::starting(self.today)),
            Action::Sample(s) => self.open_sample(s),
            Action::Edit(c) => self.edit(c),
            Action::Cloud(r) => {
                if r == crate::CloudRequest::ListProjects {
                    self.show_cloud_open();
                }
                self.cloud_requests.push(r);
            }
        }
    }

    /// The thin bar at the bottom: last message on the left.
    pub(crate) fn status_bar(&mut self, ui: &mut Ui) {
        let tok = Tokens::for_ui(ui);
        egui::Panel::bottom("status")
            .frame(
                Frame::new()
                    .fill(tok.surface)
                    .inner_margin(Margin::symmetric(14, 4)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let error = !self.status.ends_with("done") && self.status.contains(": ");
                    let colour = if error { tok.critical } else { tok.muted };
                    ui.label(RichText::new(&self.status).small().color(colour));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let k = ui
                            .ctx()
                            .format_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::K));
                        ui.label(
                            RichText::new(format!("{k}: search commands"))
                                .small()
                                .color(tok.muted),
                        );
                    });
                });
            });
    }
}

/// The app mark: an accent square with three schedule lines.
fn logo(ui: &mut Ui, tok: &Tokens) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(28.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 7.0, tok.accent);
    let s = Stroke::new(1.8, egui::Color32::WHITE);
    let o = rect.min;
    for (x0, x1, y) in [(7.0, 16.0, 9.0), (11.0, 21.0, 14.0), (8.0, 15.0, 19.0)] {
        p.line_segment([o + Vec2::new(x0, y), o + Vec2::new(x1, y)], s);
    }
}

/// A small caps group title in the navigation panel.
fn section(ui: &mut Ui, tok: &Tokens, title: &str) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        ui.label(RichText::new(title).font(semibold(11.0)).color(tok.muted));
    });
}

/// A down-pointing chevron centred on `c`.
fn chevron(p: &egui::Painter, c: Pos2, colour: egui::Color32) {
    let s = Stroke::new(1.5, colour);
    p.line_segment([c + Vec2::new(-4.0, -2.0), c + Vec2::new(0.0, 2.0)], s);
    p.line_segment([c + Vec2::new(0.0, 2.0), c + Vec2::new(4.0, -2.0)], s);
}

/// The search field of the header; it opens the command palette.
fn search_box(ui: &mut Ui, tok: &Tokens) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(280.0, 32.0), Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 8.0, ui.visuals().faint_bg_color);
    p.rect_stroke(
        rect,
        8.0,
        Stroke::new(1.0, if resp.hovered() { tok.float } else { tok.line }),
        egui::StrokeKind::Inside,
    );
    let c = Pos2::new(rect.min.x + 16.0, rect.center().y - 1.0);
    p.circle_stroke(c, 4.5, Stroke::new(1.5, tok.muted));
    p.line_segment(
        [c + Vec2::new(3.4, 3.4), c + Vec2::new(7.0, 7.0)],
        Stroke::new(1.5, tok.muted),
    );
    p.text(
        Pos2::new(rect.min.x + 30.0, rect.center().y),
        Align2::LEFT_CENTER,
        "Search commands",
        FontId::proportional(13.0),
        tok.muted,
    );
    let key = ui
        .ctx()
        .format_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::K));
    let g = p.layout_no_wrap(key, FontId::proportional(11.0), tok.muted);
    let chip = Rect::from_min_size(
        Pos2::new(rect.max.x - g.size().x - 18.0, rect.center().y - 9.0),
        Vec2::new(g.size().x + 10.0, 18.0),
    );
    p.rect_filled(chip, 4.0, tok.surface);
    p.rect_stroke(
        chip,
        4.0,
        Stroke::new(1.0, tok.line),
        egui::StrokeKind::Inside,
    );
    p.galley(
        chip.min + Vec2::new(5.0, 9.0 - g.size().y / 2.0),
        g,
        tok.muted,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// One summary card.
fn kpi(ui: &mut Ui, tok: &Tokens, label: &str, value: impl FnOnce(&mut Ui)) {
    card(tok)
        .inner_margin(Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_min_height(46.0);
            ui.label(RichText::new(label).small().color(tok.muted));
            value(ui);
        });
}

/// The large figure of a summary card.
fn big(ui: &mut Ui, tok: &Tokens, text: &str) {
    ui.label(RichText::new(text).font(semibold(19.0)).color(tok.ink));
}

/// A rounded badge tinted with `colour`.
fn pill(ui: &mut Ui, text: &str, colour: egui::Color32) {
    Frame::new()
        .fill(colour.gamma_multiply(0.14))
        .corner_radius(11)
        .inner_margin(Margin::symmetric(9, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(12.0).color(colour));
        });
}

/// A label above a value, for the inspector's facts grid.
fn fact(ui: &mut Ui, tok: &Tokens, label: &str, value: &str) {
    ui.vertical(|ui| {
        ui.label(RichText::new(label).small().color(tok.muted));
        ui.label(RichText::new(value).font(semibold(13.0)).color(tok.ink));
    });
}

/// A separated sub-section title in the inspector.
fn block(ui: &mut Ui, tok: &Tokens, title: &str) {
    ui.add_space(8.0);
    ui.separator();
    ui.label(RichText::new(title).font(semibold(12.5)).color(tok.ink));
}

/// A fixed duration as text; a three-point estimate as a range bar with
/// the most likely value marked.
fn duration_range(ui: &mut Ui, tok: &Tokens, d: &Value) {
    let Value::Object(o) = d else {
        ui.label(d.as_str().unwrap_or("–"));
        return;
    };
    let get = |k: &str| o.get(k).and_then(Value::as_str).unwrap_or("");
    let days = |s: &str| s.trim_end_matches('d').parse::<f32>().ok();
    let dist = match get("dist") {
        "pert" => "PERT",
        "uniform" => "uniform",
        _ => "triangular",
    };
    ui.label(
        RichText::new(format!("Uncertain ({dist})"))
            .small()
            .color(tok.muted),
    );
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::hover());
    let p = ui.painter();
    let track = Rect::from_min_max(
        Pos2::new(rect.min.x, rect.min.y + 8.0),
        Pos2::new(rect.max.x, rect.min.y + 12.0),
    );
    p.rect_filled(track, 2.0, tok.normal.gamma_multiply(0.35));
    let (lo, mid, hi) = (get("o"), get("m"), get("p"));
    if let (Some(a), Some(m), Some(b)) = (days(lo), days(mid), days(hi))
        && b > a
    {
        let x = rect.min.x + (m - a) / (b - a) * rect.width();
        p.circle_filled(Pos2::new(x, track.center().y), 6.0, tok.accent);
        p.text(
            Pos2::new(x, rect.max.y - 6.0),
            Align2::CENTER_CENTER,
            mid,
            semibold(11.5),
            tok.ink,
        );
    }
    let small = FontId::proportional(11.0);
    p.text(
        Pos2::new(rect.min.x, rect.max.y - 6.0),
        Align2::LEFT_CENTER,
        lo,
        small.clone(),
        tok.muted,
    );
    p.text(
        Pos2::new(rect.max.x, rect.max.y - 6.0),
        Align2::RIGHT_CENTER,
        hi,
        small,
        tok.muted,
    );
}

/// `notstarted` → `Not started`.
fn status_text(v: &Value) -> String {
    match v.as_str().unwrap_or("") {
        "notstarted" => "Not started".into(),
        "inprogress" => "In progress".into(),
        "complete" | "completed" => "Complete".into(),
        other => other.replace('_', " "),
    }
}

/// The driving chain of `schedule.activity` in words.
fn explain(a: &Value) -> String {
    let chain = a["explain"].as_array().cloned().unwrap_or_default();
    let reason = |s: &str| -> String {
        let mut parts = s.splitn(3, ' ');
        match (parts.next(), parts.next(), parts.next()) {
            (Some("FS"), Some("from"), Some(p)) => format!("starts when {p} finishes"),
            (Some("SS"), Some("from"), Some(p)) => format!("starts with {p}"),
            (Some("FF"), Some("from"), Some(p)) => format!("finishes with {p}"),
            (Some("SF"), Some("from"), Some(p)) => format!("is tied to the start of {p}"),
            _ if s == "projectstart" => "starts at the project start".into(),
            _ => s.replace('_', " "),
        }
    };
    let Some(first) = chain.first() else {
        return "Its dates come from its own constraints.".into();
    };
    let mut text = format!("It {}.", reason(first["driven_by"].as_str().unwrap_or("")));
    if chain.len() > 1 {
        let path: Vec<&str> = chain
            .iter()
            .filter_map(|c| c["activity"].as_str())
            .collect();
        let from_start = chain
            .last()
            .is_some_and(|c| c["driven_by"] == "projectstart");
        let _ = write!(
            text,
            "\nDriving chain: {}{}.",
            path.join(" ← "),
            if from_start { " ← project start" } else { "" }
        );
    }
    text
}
