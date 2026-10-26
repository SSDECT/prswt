//! Headless UI tests: run full egui frames without a window or GPU and check
//! that every view renders and that buttons drive the engine.

use std::sync::{Arc, Mutex};

use prswt_app::{FixedClock, Workspace};

use super::*;

#[test]
fn ui_01_every_tab_renders_empty_and_with_samples() {
    let mut app = app();
    let ctx = egui::Context::default();
    frame(&ctx, &mut app); // empty workspace
    for sample in prswt_app::samples::NAMES {
        app.open_sample(sample);
    }
    assert_eq!(
        app.project.as_deref(),
        Some("Unit 2 Turnaround"),
        "last opened is selected"
    );
    for tab in Tab::ALL {
        app.tab = tab;
        frame(&ctx, &mut app);
        frame(&ctx, &mut app);
        assert!(
            !app.status.contains("several projects"),
            "{tab:?}: {}",
            app.status
        );
    }
}

#[test]
fn ui_05_what_if_flow_through_ui_actions() {
    let mut app = app();
    let ctx = egui::Context::default();
    app.open_sample("bridge");
    assert!(
        app.exec("sandbox.create", json!({ "name": "if-r1" }))
            .is_some()
    );
    assert!(
        app.exec("risk.realize", json!({ "branch": "if-r1", "risk": "R1" }))
            .is_some()
    );
    app.branch = "if-r1".into();
    app.tab = Tab::Sandboxes;
    frame(&ctx, &mut app);
    let diff = app.cache.diff.clone().unwrap();
    assert_ne!(
        diff["schedule"]["finish_base"],
        diff["schedule"]["finish_head"]
    );
    assert!(
        app.exec("sandbox.commit", json!({ "sandbox": "if-r1" }))
            .is_some(),
        "humans may commit"
    );
    app.tab = Tab::Simulation;
    frame(&ctx, &mut app);
}

/// Gap 15: the Gantt draws another branch over the live plan, and the
/// comparison stops by itself when that branch goes away.
#[test]
fn dsk_11_gantt_compares_with_a_branch() {
    let mut app = app();
    let ctx = egui::Context::default();
    app.open_sample("bridge");
    assert!(
        app.exec("sandbox.create", json!({ "name": "slow" }))
            .is_some()
    );
    assert!(
        app.exec(
            "activity.update",
            json!({ "branch": "slow", "activity": "C30", "duration": "40d" })
        )
        .is_some()
    );
    let project = app.project.clone().unwrap();
    app.compare = Some((project, "slow".into()));
    app.tab = Tab::Gantt;
    frame(&ctx, &mut app);
    let other = app.compare_rows();
    let c30 = other.iter().find(|r| r["code"] == "C30").unwrap();
    let mine = app.rows();
    let here = mine.iter().find(|r| r["code"] == "C30").unwrap();
    assert_ne!(c30["early_finish"], here["early_finish"]);
    assert!(
        app.exec("sandbox.discard", json!({ "sandbox": "slow" }))
            .is_some()
    );
    app.cache = Cache::default();
    frame(&ctx, &mut app);
    frame(&ctx, &mut app);
    assert!(
        app.compare.is_none() || app.compare_rows().is_empty(),
        "a discarded branch is not compared"
    );
}

#[test]
fn dsk_02_file_round_trip_and_shortcuts_through_the_ui() {
    let mut app = app();
    let ctx = egui::Context::default();
    app.open_sample("bridge");
    let (id, name) = app.selected().unwrap();
    let rev = app.revision(&id).unwrap();
    let text = app.project_text("file.save_project").unwrap();
    assert!(
        app.project_text("file.export_xer")
            .unwrap()
            .contains("%T\tTASK")
    );
    assert!(app.close_project(&id));
    assert!(app.selected().is_none());

    assert_eq!(
        app.open_text("file.open_project", &text).as_deref(),
        Some(id.as_str())
    );
    assert_eq!(app.selected().map(|s| s.1), Some(name));
    assert_eq!(app.revision(&id), Some(rev), "reopened file is unchanged");
    assert!(app.open_text("file.open_project", "{garbage").is_none());
    assert!(app.status.contains("file.open_project"), "{}", app.status);

    // Ctrl/⌘+S and Ctrl/⌘+O become requests for the shell; Ctrl/⌘+Shift+S is Save as.
    let key = |k, m| egui::Event::Key {
        key: k,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: m,
    };
    let input = egui::RawInput {
        events: vec![
            key(egui::Key::S, egui::Modifiers::COMMAND),
            key(
                egui::Key::S,
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            ),
            key(egui::Key::O, egui::Modifiers::COMMAND),
        ],
        ..Default::default()
    };
    let mut out = ctx.run_ui(input, |ui| app.ui(ui));
    out.textures_delta.clear();
    let mut got = app.take_requests();
    got.sort_by_key(|r| format!("{r:?}"));
    assert_eq!(
        got,
        [FileRequest::Open, FileRequest::Save, FileRequest::SaveAs]
    );
    assert!(app.take_requests().is_empty());
}

#[test]
fn dsk_04_new_project_dialog_values_are_valid_for_every_preset() {
    let mut app = app();
    let mut np = NewProject::starting(jiff::civil::date(2027, 1, 4));
    assert_eq!(np.start(), "2027-01-04T08:00");
    for (i, (id, _)) in CALENDAR_PRESETS.iter().enumerate() {
        np.calendar = id;
        let r = app.exec(
            "project.create",
            json!({ "name": format!("P{i}"), "start": np.start(), "calendar": np.calendar, "project": null }),
        );
        assert!(r.is_some(), "{id}: {}", app.status);
    }
}

#[test]
// Measuring frame time needs the wall clock; only this test reads it.
#[allow(clippy::disallowed_methods)]
fn dsk_05_simulation_runs_in_the_background_while_frames_keep_drawing() {
    let mut app = app();
    let ctx = egui::Context::default();
    app.open_sample("bridge");
    app.tab = Tab::Simulation;
    app.exec_long("simulation.run", json!({ "iterations": 10_000 }));
    assert!(app.busy());
    let (mut frames, mut busy_frames) = (0, 0);
    while app.busy() || app.simulation.is_none() {
        let t = std::time::Instant::now();
        frame(&ctx, &mut app);
        assert!(t.elapsed().as_millis() < 500, "a frame blocked");
        frames += 1;
        busy_frames += usize::from(app.busy());
        assert!(frames < 100_000, "never finished: {}", app.status);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(busy_frames > 0, "at least one frame drew the busy panel");
    assert_eq!(app.simulation.as_ref().unwrap()["iterations"], 10_000);
}

/// Commands the UI calls from its own views and menus (not via a form).
const DIRECT: &[&str] = &[
    "sync.reapply",
    "project.create",
    "project.list",
    "project.lookups",
    "project.close",
    "sample.load",
    "file.open_project",
    "file.save_project",
    "file.import_xer",
    "file.export_xer",
    "file.import_mspdi",
    "file.export_mspdi",
    "edit.undo",
    "edit.redo",
    "schedule.run",
    "schedule.activities",
    "risk.list",
    "risk.realize",
    "simulation.get",
    "sandbox.create",
    "sandbox.list",
    "sandbox.diff",
    "sandbox.commit",
    "sandbox.discard",
    "resource.histogram",
    "resource.overallocation",
    // The desktop's cloud worker drives sync on local copies.
    "sync.link",
    "sync.status",
    "sync.outbox",
    "sync.apply",
];
/// Raw state-command batches are for agents and scripts, not people.
/// The server half of sync runs on the server, called by the desktop over HTTP.
const NOT_FOR_PEOPLE: &[&str] = &["commands.apply", "sync.events", "sync.receive"];

#[test]
fn dsk_06_every_command_has_a_ui_path() {
    let mut app = app();
    let list = app.exec("commands.list", json!({})).unwrap();
    // The desktop shell answers File requests, so its calls count too.
    let source = [
        include_str!("lib.rs"),
        include_str!("chrome.rs"),
        include_str!("cloud.rs"),
        include_str!("../../prswt-desktop/src/main.rs"),
        include_str!("../../prswt-desktop/src/cloud.rs"),
    ]
    .concat();
    for c in list["commands"].as_array().unwrap() {
        let id = c["id"].as_str().unwrap();
        let in_form = forms::FORMS.iter().any(|f| f.command == id);
        let direct = DIRECT.contains(&id) && source.contains(&format!("\"{id}\""));
        assert!(
            in_form || direct || NOT_FOR_PEOPLE.contains(&id),
            "`{id}` cannot be reached from the desktop UI"
        );
    }
}

#[test]
fn dsk_06_every_form_sends_parameters_the_engine_accepts() {
    // These must succeed outright on the bridge sample with default values.
    let must_succeed = [
        "Add WBS element…",
        "Add activity…",
        "Add activity with uncertain duration…",
        "Update progress…",
        "Add expense…",
        "Capture baseline…",
        "Add calendar…",
        "Calendar exception (holiday)…",
        "Add resource…",
        "Add risk…",
        "Edit risk…",
        "Correlate durations…",
        "What-if risk (inject)…",
        "Propose mitigation…",
        "Risk analysis (options)…",
        "Level resources…",
        "Check schedule",
        "Float paths…",
        "Earned value…",
        "Budget by WBS",
        "Change history",
        "Why these dates? (activity)…",
        "All engine commands",
        "Project details…",
        "Assign resource…",
        "Propose what-if branch for review…",
    ];
    for (i, spec) in forms::FORMS.iter().enumerate() {
        let mut app = app();
        app.open_sample("bridge");
        // A what-if branch, so branch pickers have something to offer.
        app.exec("sandbox.create", json!({ "name": "what-if" }));
        let lookups = app.lookups();
        let mut form = forms::OpenForm::new(spec, jiff::civil::date(2027, 6, 1), &lookups);
        form.fill_for_test(&format!("T{i}"), &lookups);
        assert!(form.complete(), "{}: incomplete after fill", spec.title);
        let ok = app.run_form(&form);
        while app.busy() {
            let ctx = egui::Context::default();
            frame(&ctx, &mut app);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let ok = ok && !app.status.contains(": ")
            || app.status.ends_with("done")
            || app.report.is_some();
        let msg = app.status.to_lowercase();
        for bad in [
            "missing required",
            "unknown parameter",
            "not a valid",
            "bad json",
            "must be an object",
        ] {
            assert!(
                !msg.contains(bad),
                "{}: params rejected: {}",
                spec.title,
                app.status
            );
        }
        if must_succeed.contains(&spec.title) {
            assert!(ok, "{} failed: {}", spec.title, app.status);
        }
    }
}

#[test]
fn dsk_06_form_builds_nested_shapes() {
    let lookups =
        json!({ "activities": [{ "code": "A", "name": "a" }, { "code": "B", "name": "b" }] });
    let spec = forms::FORMS
        .iter()
        .find(|f| f.command == "risk.add")
        .unwrap();
    let mut form = forms::OpenForm::new(spec, jiff::civil::date(2027, 6, 1), &lookups);
    form.fill_for_test("R", &lookups);
    let p = form.params();
    assert_eq!(p["probability"], 0.3);
    assert_eq!(p["impacts"][0]["delay"]["m"], "5d");
    assert_eq!(p["bindings"], json!(["global"]));
    let spec = forms::FORMS
        .iter()
        .find(|f| f.command == "calendar.exception")
        .unwrap();
    let form = forms::OpenForm::new(spec, jiff::civil::date(2027, 12, 25), &lookups);
    let p = form.params();
    assert_eq!(p["date"], "2027-12-25");
    assert_eq!(p["work"], json!([]), "first choice is a holiday");
}

fn app() -> PrswtApp {
    PrswtApp::new(Arc::new(Mutex::new(Workspace::new(Arc::new(FixedClock(
        "2027-02-01T08:00".parse().unwrap(),
    ))))))
}

fn frame(ctx: &egui::Context, app: &mut PrswtApp) {
    let mut out = ctx.run_ui(egui::RawInput::default(), |ui| app.ui(ui));
    // No renderer consumes the font atlas here; discard it explicitly.
    out.textures_delta.clear();
}
