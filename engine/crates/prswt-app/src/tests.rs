use std::sync::Arc;

use proptest::prelude::*;
use prswt_core::ErrorCode;
use serde_json::{Value, json};

use super::*;

mod workflows {
    use super::*;

    #[test]
    fn app_04_samples_load_and_schedule() {
        for name in samples::NAMES {
            let (mut w, h) = ws();
            let r = w
                .execute(&h, "sample.load", &json!({ "name": name }))
                .unwrap();
            assert!(r["finish"].as_str().is_some(), "{name}");
            let s = w.execute(&h, "schedule.run", &json!({})).unwrap();
            assert!(!s["critical_path"].as_array().unwrap().is_empty(), "{name}");
            let sim = w
                .execute(&h, "simulation.run", &json!({ "iterations": 300 }))
                .unwrap();
            assert_eq!(sim["iterations"], 300);
            assert!(
                sim["warnings"].as_array().unwrap().is_empty(),
                "{name}: {}",
                sim["warnings"]
            );
        }
    }

    #[test]
    fn dsk_01_save_then_open_in_a_fresh_workspace() {
        let (mut w, h) = ws();
        w.execute(&h, "sample.load", &json!({ "name": "bridge" }))
            .unwrap();
        w.execute(&h, "sandbox.create", &json!({ "name": "fast-track" }))
            .unwrap();
        w.execute(
            &h,
            "activity.update",
            &json!({ "activity": "C30", "duration": "20d", "branch": "fast-track" }),
        )
        .unwrap();
        let before = w
            .execute(&h, "schedule.run", &json!({ "branch": "fast-track" }))
            .unwrap();
        let saved = w.execute(&h, "file.save_project", &json!({})).unwrap();
        assert_eq!(saved["format"], "prswt");
        let text = saved["text"].as_str().unwrap();

        let err = w
            .execute(&h, "file.open_project", &json!({ "text": text }))
            .unwrap_err();
        assert!(err.message.contains("already open"), "{err}");

        let (mut fresh, h) = ws();
        let r = fresh
            .execute(&h, "file.open_project", &json!({ "text": text }))
            .unwrap();
        assert_eq!(r["project"], saved["project"]);
        assert_eq!(r["branches"], 2);
        let after = fresh
            .execute(&h, "schedule.run", &json!({ "branch": "fast-track" }))
            .unwrap();
        assert_eq!(after["finish"], before["finish"]);
        assert_eq!(after["critical_path"], before["critical_path"]);

        let rev = |w: &mut Workspace| {
            w.execute(&h, "project.list", &json!({})).unwrap()["projects"][0]["revision"].clone()
        };
        let r0 = rev(&mut fresh);
        fresh
            .execute(
                &h,
                "activity.update",
                &json!({ "activity": "C30", "duration": "21d" }),
            )
            .unwrap();
        assert_ne!(rev(&mut fresh), r0, "an edit changes the revision");
        let err = fresh
            .execute(&Ctx::agent("bot"), "project.close", &json!({}))
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::LiveWriteForbidden);
        fresh.execute(&h, "project.close", &json!({})).unwrap();
        assert_eq!(fresh.projects().count(), 0);
    }

    #[test]
    fn dsk_01_open_rejects_garbage_and_oversized_text() {
        let (mut w, h) = ws();
        for text in ["", "{", "[]", r#"{"format":"prswt-project","version":99}"#] {
            let err = w
                .execute(&h, "file.open_project", &json!({ "text": text }))
                .unwrap_err();
            assert_eq!(err.code, ErrorCode::Validation, "{text}: {err}");
        }
        let huge = " ".repeat(prswt_state::MAX_FILE_BYTES + 1);
        let err = w
            .execute(&h, "file.open_project", &json!({ "text": huge }))
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Limit);
        assert_eq!(w.projects().count(), 0);
    }

    #[test]
    fn app_02_bridge_numbers_are_consistent() {
        let (mut w, h) = ws();
        w.execute(&h, "sample.load", &json!({ "name": "bridge" }))
            .unwrap();
        let s = w.execute(&h, "schedule.run", &json!({})).unwrap();
        assert_eq!(
            s["critical_path"],
            json!([
                "M0", "D10", "D20", "D30", "C10", "C20", "C30", "C40", "C50", "C60", "X10", "M9"
            ])
        );
        let cost = w.execute(&h, "cost.summary", &json!({})).unwrap();
        assert!(cost["total"].as_str().unwrap().parse::<f64>().unwrap() > 570_000.0);
        let evm = w.execute(&h, "evm.report", &json!({})).unwrap();
        assert_eq!(num(&evm["pv"]), 0.0, "nothing planned before the start");
        let health = w.execute(&h, "schedule.health", &json!({})).unwrap();
        assert_eq!(health["checks"].as_array().unwrap().len(), 14);
        let a = w
            .execute(&h, "schedule.activity", &json!({ "activity": "C40" }))
            .unwrap();
        assert_eq!(a["predecessors"].as_array().unwrap().len(), 2);
        assert_eq!(a["explain"][0]["activity"], "C40");
    }

    #[test]
    fn app_02_agent_what_if_then_human_commit() {
        let (mut w, h) = ws();
        let agent = Ctx::agent("risk-auditor");
        w.execute(&h, "sample.load", &json!({ "name": "bridge" }))
            .unwrap();
        let err = w
            .execute(
                &agent,
                "activity.update",
                &json!({ "activity": "C30", "duration": "20d" }),
            )
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::LiveWriteForbidden);
        w.execute(
            &agent,
            "sandbox.create",
            &json!({ "name": "permit-slip", "ttl_minutes": 120 }),
        )
        .unwrap();
        let r = w
            .execute(
                &agent,
                "risk.realize",
                &json!({ "branch": "permit-slip", "risk": "R1", "choice": "pessimistic" }),
            )
            .unwrap();
        assert_eq!(r["applied"], true);
        assert_eq!(
            r["delta"]["finish_shift"], "20d",
            "permit +20d on the critical path"
        );
        let sim = w
            .execute(
                &agent,
                "simulation.run",
                &json!({ "branch": "permit-slip", "iterations": 400, "compare_to": "live" }),
            )
            .unwrap();
        assert!(sim["delta_vs"]["finish_days"]["p50"].as_f64().unwrap() > 0.0);
        let d = w
            .execute(&agent, "sandbox.diff", &json!({ "head": "permit-slip" }))
            .unwrap();
        assert!(!d["changes"].as_array().unwrap().is_empty());
        assert_eq!(
            w.execute(
                &agent,
                "sandbox.commit",
                &json!({ "sandbox": "permit-slip" })
            )
            .unwrap_err()
            .code,
            ErrorCode::LiveWriteForbidden
        );
        let before = w.execute(&h, "schedule.run", &json!({})).unwrap()["finish"].clone();
        w.execute(
            &h,
            "sandbox.commit",
            &json!({ "sandbox": "permit-slip", "capture_baseline": "BL2" }),
        )
        .unwrap();
        let after = w.execute(&h, "schedule.run", &json!({})).unwrap()["finish"].clone();
        assert_ne!(before, after);
        let risks = w.execute(&h, "risk.list", &json!({})).unwrap();
        let r1 = risks["risks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["code"] == "R1")
            .unwrap();
        assert_eq!(r1["status"], "realized");
        w.execute(&h, "edit.undo", &json!({})).unwrap(); // baseline capture
        w.execute(&h, "edit.undo", &json!({})).unwrap(); // merge
        assert_eq!(
            w.execute(&h, "schedule.run", &json!({})).unwrap()["finish"],
            before
        );
    }

    #[test]
    fn app_02_mitigation_reduces_p80() {
        let (mut w, h) = ws();
        w.execute(&h, "sample.load", &json!({ "name": "bridge" }))
            .unwrap();
        w.execute(&h, "sandbox.create", &json!({ "name": "crash" }))
            .unwrap();
        let prop = w
            .execute(&h, "mitigation.propose", &json!({ "branch": "crash", "risk": "R1", "strategy": "crash", "pct": 30, "cost_per_day": 1500 }))
            .unwrap();
        assert_eq!(prop["edits"].as_array().unwrap().len(), 1);
        w.execute(&h, "mitigation.attach", &json!({ "branch": "crash", "risk": "R1", "strategy": "crash", "pct": 30, "cost_per_day": 1500, "post_probability": 0.1 }))
            .unwrap();
        let sim = w
            .execute(
                &h,
                "simulation.run",
                &json!({ "branch": "crash", "iterations": 600, "compare_to": "live" }),
            )
            .unwrap();
        assert!(
            sim["delta_vs"]["finish_days"]["p80"].as_f64().unwrap() < 0.0,
            "{}",
            sim["delta_vs"]
        );
    }

    #[test]
    fn app_02_build_progress_level_and_evm() {
        let (mut w, h) = ws();
        w.execute(
            &h,
            "project.create",
            &json!({ "name": "Demo", "start": "2027-01-04T08:00" }),
        )
        .unwrap();
        w.execute(
            &h,
            "activity.add",
            &json!({ "code": "A", "duration": "4d" }),
        )
        .unwrap();
        w.execute(
            &h,
            "activity.add",
            &json!({ "code": "B", "duration": "4d" }),
        )
        .unwrap();
        w.execute(
            &h,
            "activity.add",
            &json!({ "code": "C", "duration": "2d", "predecessors": ["A", "B"] }),
        )
        .unwrap();
        w.execute(
            &h,
            "resource.add",
            &json!({ "code": "R", "max_units": 1, "rate": 100 }),
        )
        .unwrap();
        w.execute(
            &h,
            "resource.assign",
            &json!({ "activity": "A", "resource": "R" }),
        )
        .unwrap();
        w.execute(
            &h,
            "resource.assign",
            &json!({ "activity": "B", "resource": "R" }),
        )
        .unwrap();
        let over = w
            .execute(&h, "resource.overallocation", &json!({}))
            .unwrap();
        assert_eq!(over["overallocations"].as_array().unwrap().len(), 1);
        let lv = w
            .execute(&h, "resource.level", &json!({ "apply": true }))
            .unwrap();
        assert_eq!(
            lv["finish_after"], "2027-01-15T17:00",
            "B pushed to Fri-Wed, C Thu-Fri"
        );
        assert_eq!(
            w.execute(&h, "schedule.run", &json!({})).unwrap()["finish"],
            "2027-01-15T17:00"
        );
        w.execute(&h, "baseline.capture", &json!({ "name": "BL1" }))
            .unwrap();
        w.execute(
            &h,
            "activity.progress",
            &json!({ "activity": "A", "actual_start": "2027-01-04T08:00", "pct": 50 }),
        )
        .unwrap();
        w.execute(
            &h,
            "project.update",
            &json!({ "data_date": "2027-01-06T08:00" }),
        )
        .unwrap();
        let evm = w.execute(&h, "evm.report", &json!({})).unwrap();
        assert_eq!(num(&evm["pv"]), 1600.0, "2 days x 8h x 100");
        assert_eq!(num(&evm["ev"]), 1600.0, "50% of 3200");
    }

    #[test]
    fn app_02_unknown_and_bad_input_errors() {
        let (mut w, h) = ws();
        assert_eq!(
            w.execute(&h, "nope", &json!({})).unwrap_err().code,
            ErrorCode::NotFound
        );
        assert_eq!(
            w.execute(&h, "schedule.run", &json!({})).unwrap_err().code,
            ErrorCode::NotFound
        );
        w.execute(
            &h,
            "project.create",
            &json!({ "name": "X", "start": "2027-01-04" }),
        )
        .unwrap();
        let e = w
            .execute(
                &h,
                "activity.add",
                &json!({ "code": "A", "duration": "abc" }),
            )
            .unwrap_err();
        assert_eq!(e.code, ErrorCode::Validation);
        let e = w
            .execute(&h, "activity.add", &json!({ "code": 7 }))
            .unwrap_err();
        assert!(e.message.contains("string"));
        let list = w
            .execute(&h, "commands.list", &json!({ "search": "sandbox" }))
            .unwrap();
        assert_eq!(list["commands"].as_array().unwrap().len(), 6);
    }
}

// ───────────────────────────── APP-03 fuzzing ─────────────────────────────

/// GridCraft's `every_command_survives_empty_params`: no command may panic
/// (Internal) on empty or garbage params, and integrity must hold after.
#[test]
fn app_03_every_command_survives_empty_and_garbage_params() {
    let garbage = [
        json!({}),
        json!(null),
        json!([1, 2]),
        json!({ "activity": "C30", "duration": -5, "pct": 1e308, "name": "" }),
        json!({ "project": 42, "branch": { "x": 1 }, "iterations": u64::MAX, "rho": "2" }),
        json!({ "commands": [{ "op": "delete_activity", "id": "not-a-uuid" }] }),
        json!({ "date": "9999-99-99", "start": "0000-01-01", "lag": "999999999d" }),
    ];
    for spec in catalog() {
        for params in &garbage {
            let (mut w, h) = ws();
            w.execute(&h, "sample.load", &json!({ "name": "bridge" }))
                .unwrap();
            if let Err(e) = w.execute(&h, spec.id, params) {
                assert_ne!(
                    e.code,
                    ErrorCode::Internal,
                    "{} panicked on {params}",
                    spec.id
                );
            }
            for repo in w.projects() {
                repo.branch(repo.live)
                    .unwrap()
                    .head
                    .check_integrity()
                    .unwrap();
            }
        }
    }
}

fn arb_value() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        (-1_000i64..1_000).prop_map(|n| json!(n)),
        prop::sample::select(vec![
            "C30",
            "D20",
            "R1",
            "R3",
            "CREW",
            "live",
            "s1",
            "5d",
            "-2d",
            "FS",
            "SF",
            "2027-04-01",
            "pert",
            "crash",
            "x"
        ])
        .prop_map(|s| json!(s)),
    ];
    let keys = prop::sample::select(vec![
        "activity",
        "pred",
        "succ",
        "kind",
        "lag",
        "duration",
        "risk",
        "target",
        "probability",
        "branch",
        "name",
        "code",
        "strategy",
        "pct",
        "sandbox",
        "head",
        "resource",
        "units",
        "date",
        "apply",
        "iterations",
    ]);
    proptest::collection::btree_map(keys, leaf, 0..6)
        .prop_map(|m| Value::Object(m.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Random command sequences never panic and never break integrity.
    #[test]
    fn app_03_random_command_sequences(steps in proptest::collection::vec((0usize..64, arb_value()), 1..25)) {
        let (mut w, h) = ws();
        w.execute(&h, "sample.load", &json!({ "name": "bridge" })).unwrap();
        w.execute(&h, "sandbox.create", &json!({ "name": "s1" })).unwrap();
        let cat = catalog();
        for (k, params) in steps {
            let spec = cat[k % cat.len()];
            if spec.id == "simulation.run" {
                continue; // covered separately; keeps the fuzz fast
            }
            if let Err(e) = w.execute(&h, spec.id, &params) {
                prop_assert_ne!(e.code, ErrorCode::Internal, "{} on {}", spec.id, params);
            }
        }
        for repo in w.projects() {
            for b in repo.branches() {
                prop_assert!(b.head.check_integrity().is_ok());
            }
        }
    }
}

/// SYNC-01..03: a server and desktop copies, all in one process; the JSON
/// they exchange is exactly what travels over HTTP.
mod cloud {
    use super::*;

    const SERVER: &str = "https://prswt.example";

    #[test]
    fn sync_01_upload_then_download_starts_in_sync() {
        let (mut server, mut a, owner) = shared_project();
        let st = a.execute(&owner, "sync.status", &json!({})).unwrap();
        assert_eq!(st["linked"], true);
        assert_eq!(st["pending_branches"], 0);
        let mut b = download(&mut server, &owner);
        let st = b.execute(&owner, "sync.status", &json!({})).unwrap();
        assert_eq!(st["pending_branches"], 0);
        assert_eq!(head(&a), head(&b));
    }

    #[test]
    fn sync_02_local_edit_reaches_the_server_and_other_copies() {
        let (mut server, mut a, owner) = shared_project();
        let mut b = download(&mut server, &owner);
        a.execute(
            &owner,
            "activity.update",
            &json!({ "activity": "D10", "name": "Survey and soil tests" }),
        )
        .unwrap();
        assert_eq!(
            a.execute(&owner, "sync.status", &json!({})).unwrap()["pending_branches"],
            1
        );
        sync(&mut a, &mut server, &owner).unwrap();
        assert_eq!(
            a.execute(&owner, "sync.status", &json!({})).unwrap()["pending_branches"],
            0
        );
        sync(&mut b, &mut server, &owner).unwrap();
        assert_eq!(name_of(&b, "D10"), "Survey and soil tests");
        assert_eq!(head(&a), head(&server_copy(&server)));
        assert_eq!(head(&b), head(&server_copy(&server)));
    }

    #[test]
    fn sync_02_concurrent_edits_to_different_things_both_survive() {
        let (mut server, mut a, owner) = shared_project();
        let mut b = download(&mut server, &owner);
        a.execute(
            &owner,
            "activity.update",
            &json!({ "activity": "D10", "duration": "7d" }),
        )
        .unwrap();
        b.execute(
            &owner,
            "activity.update",
            &json!({ "activity": "C20", "name": "Demolish old deck" }),
        )
        .unwrap();
        sync(&mut a, &mut server, &owner).unwrap();
        // B's push is refused (the server moved); B pulls, rebases, retries.
        let first = push(&mut b, &mut server, &owner);
        assert_eq!(first.unwrap_err().code, ErrorCode::VersionConflict);
        let conflicts = sync(&mut b, &mut server, &owner).unwrap();
        assert!(conflicts.is_empty(), "{conflicts:?}");
        sync(&mut a, &mut server, &owner).unwrap();
        for copy in [&a, &b] {
            assert_eq!(name_of(copy, "C20"), "Demolish old deck");
            assert_eq!(head(copy), head(&server_copy(&server)));
        }
    }

    #[test]
    fn sync_02_same_field_changed_twice_keeps_the_server_value_and_reports_it() {
        let (mut server, mut a, owner) = shared_project();
        let mut b = download(&mut server, &owner);
        a.execute(
            &owner,
            "activity.update",
            &json!({ "activity": "D10", "name": "From A" }),
        )
        .unwrap();
        b.execute(
            &owner,
            "activity.update",
            &json!({ "activity": "D10", "name": "From B" }),
        )
        .unwrap();
        sync(&mut a, &mut server, &owner).unwrap();
        let conflicts = sync(&mut b, &mut server, &owner).unwrap();
        assert_eq!(conflicts.len(), 1, "{conflicts:?}");
        assert_eq!(conflicts[0]["field"], "name");
        assert_eq!(conflicts[0]["server"], "From A");
        assert_eq!(conflicts[0]["yours"], "From B");
        assert_eq!(name_of(&b, "D10"), "From A");
        assert_eq!(head(&b), head(&server_copy(&server)));
    }

    #[test]
    fn sync_02_edit_of_a_deleted_activity_is_dropped_with_a_report() {
        let (mut server, mut a, owner) = shared_project();
        let mut b = download(&mut server, &owner);
        a.execute(
            &owner,
            "activity.delete",
            &json!({ "activity": "X10", "reconnect": true }),
        )
        .unwrap();
        b.execute(
            &owner,
            "activity.update",
            &json!({ "activity": "X10", "name": "Final walk-through" }),
        )
        .unwrap();
        sync(&mut a, &mut server, &owner).unwrap();
        let conflicts = sync(&mut b, &mut server, &owner).unwrap();
        assert!(!conflicts.is_empty());
        assert!(
            b.execute(&owner, "schedule.activity", &json!({ "activity": "X10" }))
                .is_err()
        );
        assert_eq!(head(&b), head(&server_copy(&server)));
    }

    #[test]
    fn sync_03_offline_sandbox_and_proposal_reach_reviewers() {
        let (mut server, mut a, owner) = shared_project();
        let mut b = download(&mut server, &owner);
        let agent = Ctx::agent("claude");
        a.execute(&agent, "sandbox.create", &json!({ "name": "fast-track" }))
            .unwrap();
        a.execute(
            &agent,
            "activity.update",
            &json!({ "branch": "fast-track", "activity": "C30", "duration": "10d" }),
        )
        .unwrap();
        a.execute(
            &agent,
            "sandbox.propose",
            &json!({ "sandbox": "fast-track", "note": "Shorter foundations" }),
        )
        .unwrap();
        sync(&mut a, &mut server, &owner).unwrap();
        sync(&mut b, &mut server, &owner).unwrap();
        let st = b.execute(&owner, "sync.status", &json!({})).unwrap();
        let sb = st["branches"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == "fast-track")
            .unwrap();
        assert_eq!(sb["proposal"]["note"], "Shorter foundations");
        // A person accepts it on B; the commit reaches the server and A.
        b.execute(
            &owner,
            "sandbox.commit",
            &json!({ "sandbox": "fast-track" }),
        )
        .unwrap();
        sync(&mut b, &mut server, &owner).unwrap();
        sync(&mut a, &mut server, &owner).unwrap();
        let on_server = server_copy(&server);
        let live = on_server
            .repo(on_server.projects().next().unwrap().project)
            .unwrap();
        let fast = live
            .branches()
            .find(|x| x.meta.name == "fast-track")
            .unwrap();
        assert_eq!(fast.meta.status, prswt_state::BranchStatus::Merged);
        assert_eq!(head(&a), head(&on_server));
    }

    #[test]
    fn sync_03_agents_cannot_commit_to_live_through_sync() {
        let (mut server, mut a, owner) = shared_project();
        let agent = Ctx::agent("claude").with_access(full_access(&server));
        a.execute(&owner, "sandbox.create", &json!({ "name": "s" }))
            .unwrap();
        sync(&mut a, &mut server, &owner).unwrap();
        a.execute(&owner, "sandbox.commit", &json!({ "sandbox": "s" }))
            .unwrap();
        let out = a.execute(&owner, "sync.outbox", &json!({})).unwrap();
        let row = out["branches"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["meta"]["name"] == "s")
            .unwrap()
            .clone();
        let mut req = row;
        req["project"] = project_id(&server);
        let err = server.execute(&agent, "sync.receive", &req).unwrap_err();
        assert_eq!(err.code, ErrorCode::LiveWriteForbidden);
    }

    #[test]
    fn sync_01_scoped_callers_see_only_their_projects_and_roles_hold() {
        let (mut server, _a, owner) = shared_project();
        let pid = project_id(&server);
        let created = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = created.clone();
        server.on_create = Some(Box::new(move |_, p| {
            seen.lock().unwrap().push(p);
            Ok(())
        }));
        // A stranger sees nothing, cannot address the project by name or id.
        let stranger = Ctx::human("eve@other.example").with_access(Access::new());
        let list = server
            .execute(&stranger, "project.list", &json!({ "brief": true }))
            .unwrap();
        assert!(list["projects"].as_array().unwrap().is_empty());
        let err = server
            .execute(&stranger, "schedule.run", &json!({ "project": pid }))
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
        // A viewer reads but cannot write; an editor cannot close.
        let viewer = Ctx::human("vic@juz.ai").with_access(Access::from([(id(&pid), Role::Viewer)]));
        server.execute(&viewer, "schedule.run", &json!({})).unwrap();
        let err = server
            .execute(
                &viewer,
                "activity.update",
                &json!({ "activity": "D10", "name": "x" }),
            )
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Forbidden);
        let editor = Ctx::human("ed@juz.ai").with_access(Access::from([(id(&pid), Role::Editor)]));
        server
            .execute(
                &editor,
                "activity.update",
                &json!({ "activity": "D10", "name": "x" }),
            )
            .unwrap();
        let err = server
            .execute(&editor, "project.close", &json!({}))
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Forbidden);
        // Creating a project reports it to the host; re-uploading someone
        // else's project id is refused and leaves theirs untouched.
        server
            .execute(&stranger, "sample.load", &json!({ "name": "turnaround" }))
            .unwrap();
        assert_eq!(created.lock().unwrap().len(), 1);
        let text = server
            .execute(
                &owner_ctx(&server),
                "file.save_project",
                &json!({ "project": pid }),
            )
            .unwrap()["text"]
            .clone();
        let before = head(&server_copy(&server));
        let err = server
            .execute(&stranger, "file.open_project", &json!({ "text": text }))
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Validation);
        assert_eq!(server.projects().count(), 2);
        let _ = owner;
        assert_eq!(head(&server_copy(&server)), before);
    }

    /// Review C1: an offline link to an activity someone deleted on the
    /// server no longer fits. The sync still succeeds, reports it, takes
    /// the server's state and keeps the other offline work.
    #[test]
    fn sync_06_offline_change_that_no_longer_fits_is_reported_not_lost() {
        let (mut server, mut a, owner) = shared_project();
        let mut b = download(&mut server, &owner);
        b.execute(
            &owner,
            "activity.delete",
            &json!({ "activity": "X10", "reconnect": true }),
        )
        .unwrap();
        sync(&mut b, &mut server, &owner).unwrap();
        a.execute(
            &owner,
            "relationship.add",
            &json!({ "pred": "D10", "succ": "X10", "kind": "SS" }),
        )
        .unwrap();
        a.execute(
            &owner,
            "activity.update",
            &json!({ "activity": "D10", "name": "Kept offline" }),
        )
        .unwrap();
        let conflicts = sync(&mut a, &mut server, &owner).unwrap();
        assert!(
            conflicts.iter().any(|c| c["field"] == "*"),
            "the dropped link is reported: {conflicts:?}"
        );
        assert_eq!(name_of(&a, "D10"), "Kept offline", "other work survives");
        assert_eq!(head(&a), head(&server_copy(&server)));
    }

    /// Review C1: a sync that fails part-way (a gap in the server's
    /// events) leaves the copy and its unsent work exactly as they were.
    #[test]
    fn sync_06_failed_apply_keeps_local_work() {
        let (mut server, mut a, owner) = shared_project();
        let mut b = download(&mut server, &owner);
        for name in ["one", "two"] {
            b.execute(
                &owner,
                "activity.update",
                &json!({ "activity": "C20", "name": name }),
            )
            .unwrap();
            sync(&mut b, &mut server, &owner).unwrap();
        }
        a.execute(
            &owner,
            "activity.update",
            &json!({ "activity": "D10", "name": "Offline" }),
        )
        .unwrap();
        // The first server event after the agreed version goes missing.
        let before = head(&a);
        let mut pulled = server
            .execute(&owner, "sync.events", &json!({ "since": {} }))
            .unwrap();
        let live = pulled["branches"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|r| r["meta"]["kind"] == "live")
            .unwrap();
        live["events"]
            .as_array_mut()
            .unwrap()
            .retain(|e| e["version"] != json!(live_version(&a)));
        let err = a
            .execute(
                &owner,
                "sync.apply",
                &json!({ "branches": pulled["branches"] }),
            )
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Validation, "{err:?}");
        assert_eq!(head(&a), before, "nothing was rewound");
        assert_eq!(
            a.execute(&owner, "sync.status", &json!({})).unwrap()["pending_branches"],
            1,
            "the offline edit is still waiting to be sent"
        );
        sync(&mut a, &mut server, &owner).unwrap();
        assert_eq!(name_of(&a, "D10"), "Offline");
        assert_eq!(name_of(&a, "C20"), "two");
    }

    /// Review M7: after someone else's events arrive, undo cannot reach
    /// past them and overwrite their work.
    #[test]
    fn sync_06_undo_does_not_overwrite_arrived_changes() {
        let (mut server, mut a, owner) = shared_project();
        let mut b = download(&mut server, &owner);
        a.execute(
            &owner,
            "activity.update",
            &json!({ "activity": "D10", "name": "Mine" }),
        )
        .unwrap();
        sync(&mut a, &mut server, &owner).unwrap();
        sync(&mut b, &mut server, &owner).unwrap();
        b.execute(
            &owner,
            "activity.update",
            &json!({ "activity": "D10", "name": "Theirs" }),
        )
        .unwrap();
        sync(&mut b, &mut server, &owner).unwrap();
        sync(&mut a, &mut server, &owner).unwrap();
        assert!(a.execute(&owner, "edit.undo", &json!({})).is_err());
        assert_eq!(name_of(&a, "D10"), "Theirs");
    }

    /// Review C2: a whole-entity write cannot file an entity under another
    /// id or move the project to another id.
    #[test]
    fn sync_06_set_entity_keeps_ids() {
        let (mut w, ctx) = ws();
        w.execute(&ctx, "sample.load", &json!({ "name": "bridge" }))
            .unwrap();
        let before = head(&w);
        let repo = w.projects().next().unwrap();
        let live = repo.branch(repo.live).unwrap().clone();
        let mut acts = live.head.activities.values();
        let (first, second) = (acts.next().unwrap().clone(), acts.next().unwrap().clone());
        let mut moved = serde_json::to_value(&second).unwrap();
        moved["code"] = json!("ZZ1");
        let cmd = prswt_state::Command::SetEntity {
            entity: prswt_state::EntityRef::new(prswt_state::EntityKind::Activity, first.id),
            value: Some(moved),
        };
        let err = w
            .execute(&ctx, "commands.apply", &json!({ "commands": [cmd] }))
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Validation, "{err:?}");
        let mut project = serde_json::to_value(&live.head.project).unwrap();
        project["id"] = json!(prswt_core::ids::ProjectId::new());
        let cmd = prswt_state::Command::SetEntity {
            entity: prswt_state::EntityRef::project(),
            value: Some(project),
        };
        let err = w
            .execute(&ctx, "commands.apply", &json!({ "commands": [cmd] }))
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Validation, "{err:?}");
        assert_eq!(head(&w), before);
    }

    /// Review H1, H2, H6, M6, M8: sandboxes need an editor, read-only
    /// callers write nothing, local-copy commands are refused on a server,
    /// a write must name a project, and a refused new project is not kept.
    #[test]
    fn sync_06_roles_cover_every_write() {
        let (mut server, _a, owner) = shared_project();
        let pid = project_id(&server);
        let viewer = Ctx::human("vic@juz.ai").with_access(Access::from([(id(&pid), Role::Viewer)]));
        for (cmd, params) in [
            ("sandbox.create", json!({ "name": "spam" })),
            ("sandbox.discard", json!({ "sandbox": "x" })),
        ] {
            let err = server.execute(&viewer, cmd, &params).unwrap_err();
            assert_eq!(err.code, ErrorCode::Forbidden, "{cmd}");
        }
        let reader = Ctx::agent("reader").read_only();
        for (cmd, params) in [
            ("sample.load", json!({ "name": "turnaround" })),
            ("sandbox.create", json!({ "name": "r" })),
        ] {
            let err = server.execute(&reader, cmd, &params).unwrap_err();
            assert_eq!(err.code, ErrorCode::Forbidden, "{cmd}");
        }
        server.execute(&reader, "schedule.run", &json!({})).unwrap();
        let o = owner_ctx(&server);
        for cmd in ["sync.apply", "sync.link", "sync.outbox"] {
            let err = server
                .execute(&o, cmd, &json!({ "server": SERVER }))
                .unwrap_err();
            assert_eq!(err.code, ErrorCode::Forbidden, "{cmd}");
        }
        let err = server
            .execute(
                &o,
                "activity.update",
                &json!({ "project": "nope", "activity": "D10", "name": "x" }),
            )
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound, "no fail-open");
        server.on_create = Some(Box::new(|_, _| {
            Err(EngineError::validation("owner could not be recorded"))
        }));
        let n = server.projects().count();
        assert!(
            server
                .execute(&o, "sample.load", &json!({ "name": "turnaround" }))
                .is_err()
        );
        assert_eq!(server.projects().count(), n, "no orphan project");
        let _ = owner;
    }

    /// Review H2: a branch from the server must be a sandbox of this
    /// project (a second live branch would make the file unreadable).
    #[test]
    fn sync_06_arriving_branch_must_be_a_sandbox() {
        let (mut server, mut a, owner) = shared_project();
        server
            .execute(&owner, "sandbox.create", &json!({ "name": "s" }))
            .unwrap();
        let mut pulled = server
            .execute(&owner, "sync.events", &json!({ "since": {} }))
            .unwrap();
        for row in pulled["branches"].as_array_mut().unwrap() {
            if row["meta"]["name"] == "s" {
                row["meta"]["kind"] = json!("live");
            }
        }
        let err = a
            .execute(
                &owner,
                "sync.apply",
                &json!({ "branches": pulled["branches"] }),
            )
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Validation);
        assert_eq!(a.projects().next().unwrap().branches().count(), 1);
    }

    /// Review M9: a closed sandbox is not reopened by a client, and a
    /// review request is recorded as made by the caller.
    #[test]
    fn sync_06_server_owns_sandbox_status() {
        let (mut server, mut a, owner) = shared_project();
        let o = owner.clone();
        a.execute(&owner, "sandbox.create", &json!({ "name": "s" }))
            .unwrap();
        sync(&mut a, &mut server, &owner).unwrap();
        a.execute(
            &Ctx::agent("claude"),
            "sandbox.propose",
            &json!({ "sandbox": "s", "note": "Please look" }),
        )
        .unwrap();
        sync(&mut a, &mut server, &owner).unwrap();
        let on_server = server.execute(&o, "sandbox.list", &json!({})).unwrap();
        let s = on_server["branches"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["name"] == "s")
            .unwrap()
            .clone();
        assert!(!s["proposal"]["by"].to_string().contains("claude"), "{s}");
        assert_eq!(
            a.execute(&owner, "sync.status", &json!({})).unwrap()["pending_branches"],
            0,
            "the copy takes the server's record and settles"
        );
        server
            .execute(&o, "sandbox.discard", &json!({ "sandbox": "s" }))
            .unwrap();
        let out = a.execute(&owner, "sync.outbox", &json!({})).unwrap();
        assert!(out["branches"].as_array().unwrap().is_empty());
        let st = a.execute(&owner, "sync.status", &json!({})).unwrap();
        let row = st["branches"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["name"] == "s")
            .unwrap()
            .clone();
        let req = json!({
            "project": pid_of(&server),
            "meta": a.projects().next().unwrap().branches().find(|b| b.meta.name == "s").map(|b| serde_json::to_value(&b.meta).unwrap()).unwrap(),
            "base_version": row["version"],
        });
        let err = server.execute(&o, "sync.receive", &req).unwrap_err();
        assert_eq!(
            err.code,
            ErrorCode::Validation,
            "a discarded sandbox stays closed"
        );
    }

    // ── helpers ──

    fn live_version(w: &Workspace) -> u64 {
        let repo = w.projects().next().unwrap();
        repo.branch(repo.live).unwrap().meta.version
    }

    fn pid_of(w: &Workspace) -> Value {
        project_id(w)
    }

    /// A server holding the bridge sample owned by `owner`, and copy A
    /// uploaded from a desktop.
    fn shared_project() -> (Workspace, Workspace, Ctx) {
        let (mut a, owner) = ws();
        a.execute(&owner, "sample.load", &json!({ "name": "bridge" }))
            .unwrap();
        let text = a.execute(&owner, "file.save_project", &json!({})).unwrap()["text"].clone();
        let (mut server, _) = ws();
        server
            .execute(&owner, "file.open_project", &json!({ "text": text }))
            .unwrap();
        a.execute(&owner, "sync.link", &json!({ "server": SERVER }))
            .unwrap();
        (server, a, owner)
    }

    /// A fresh desktop copy downloaded from the server.
    fn download(server: &mut Workspace, ctx: &Ctx) -> Workspace {
        let text = server
            .execute(ctx, "file.save_project", &json!({}))
            .unwrap()["text"]
            .clone();
        let (mut w, _) = ws();
        w.execute(ctx, "file.open_project", &json!({ "text": text }))
            .unwrap();
        w.execute(ctx, "sync.link", &json!({ "server": SERVER }))
            .unwrap();
        w
    }

    /// One push of every unsent branch; stops at the first refusal.
    fn push(copy: &mut Workspace, server: &mut Workspace, ctx: &Ctx) -> Result<(), EngineError> {
        let out = copy.execute(ctx, "sync.outbox", &json!({}))?;
        for row in out["branches"].as_array().cloned().unwrap_or_default() {
            let mut req = row.clone();
            req["project"] = out["project"].clone();
            let reply = server.execute(ctx, "sync.receive", &req)?;
            copy.execute(
                ctx,
                "sync.apply",
                &json!({ "branches": [{ "meta": reply["meta"], "events": reply["events"] }] }),
            )?;
        }
        Ok(())
    }

    /// The desktop loop: pull, then push (retrying after a refusal).
    /// Returns the conflicts reported while rebasing.
    fn sync(
        copy: &mut Workspace,
        server: &mut Workspace,
        ctx: &Ctx,
    ) -> Result<Vec<Value>, EngineError> {
        let mut conflicts = Vec::new();
        for _ in 0..3 {
            let st = copy.execute(ctx, "sync.status", &json!({}))?;
            let since: serde_json::Map<String, Value> = st["branches"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|b| !b["synced"].is_null())
                .map(|b| (b["id"].as_str().unwrap().to_owned(), b["synced"].clone()))
                .collect();
            let pulled = server.execute(
                ctx,
                "sync.events",
                &json!({ "project": st["project"], "since": since }),
            )?;
            let applied = copy.execute(
                ctx,
                "sync.apply",
                &json!({ "branches": pulled["branches"] }),
            )?;
            conflicts.extend(applied["conflicts"].as_array().cloned().unwrap_or_default());
            match push(copy, server, ctx) {
                Ok(()) => return Ok(conflicts),
                Err(e) if e.code == ErrorCode::VersionConflict => {}
                Err(e) => return Err(e),
            }
        }
        Err(EngineError::validation("sync did not settle"))
    }

    fn head(w: &Workspace) -> Value {
        let repo = w.projects().next().unwrap();
        let mut heads: Vec<(String, Value)> = repo
            .branches()
            .map(|b| (b.meta.name.clone(), serde_json::to_value(&b.head).unwrap()))
            .collect();
        heads.sort_by(|x, y| x.0.cmp(&y.0));
        json!(heads)
    }

    fn server_copy(server: &Workspace) -> Workspace {
        let (mut w, _) = ws();
        for r in server.projects() {
            w.insert_repo(r.clone());
        }
        w
    }

    fn name_of(w: &Workspace, code: &str) -> String {
        let repo = w.projects().next().unwrap();
        let live = repo.branch(repo.live).unwrap();
        live.head
            .activities
            .values()
            .find(|a| a.code == code)
            .unwrap()
            .name
            .clone()
    }

    fn project_id(w: &Workspace) -> Value {
        json!(w.projects().next().unwrap().project)
    }

    fn id(v: &Value) -> prswt_core::ids::ProjectId {
        v.as_str().unwrap().parse().unwrap()
    }

    fn full_access(w: &Workspace) -> Access {
        w.projects().map(|r| (r.project, Role::Owner)).collect()
    }

    fn owner_ctx(w: &Workspace) -> Ctx {
        Ctx::human("owner@juz.ai").with_access(full_access(w))
    }
}

fn num(v: &Value) -> f64 {
    v.as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| v.as_f64())
        .unwrap()
}

fn ws() -> (Workspace, Ctx) {
    let clock: Arc<dyn Clock> = Arc::new(FixedClock("2027-02-01T08:00".parse().unwrap()));
    (Workspace::new(clock), Ctx::human("planner"))
}
