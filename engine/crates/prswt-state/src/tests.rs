use std::collections::BTreeMap;

use proptest::prelude::*;
use prswt_core::ids::{ActivityId, CalendarId, ProjectId, RelationshipId, WbsId};
use prswt_core::model::{
    Activity, ActivityKind, CalendarDef, Estimate, PercentCompleteType, Project, RelationKind,
    Relationship, ScheduleSettings, WbsNode, WorkWeek,
};
use prswt_core::{ErrorCode, Instant, WorkMinutes};
use rust_decimal::Decimal;

use super::*;

const T0: Instant = Instant(14_000_000);

mod commands {
    use super::*;

    #[test]
    fn st_02_batches_are_atomic_and_validated() {
        let mut repo = repo();
        let live = repo.live;
        let a = act(&repo, "A");
        let mut orphan = act(&repo, "X");
        orphan.wbs = WbsId::new();
        let err = repo
            .execute(live, vec![upsert(a), upsert(orphan)], me(), T0, None)
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Validation);
        assert_eq!(
            repo.branch(live).unwrap().meta.version,
            0,
            "nothing applied"
        );
        assert!(repo.branch(live).unwrap().head.activities.is_empty());
    }

    #[test]
    fn st_02_cycles_and_version_conflicts() {
        let mut repo = repo();
        let live = repo.live;
        let (a, b) = (act(&repo, "A"), act(&repo, "B"));
        let (ia, ib) = (a.id, b.id);
        repo.execute(
            live,
            vec![upsert(a), upsert(b), link(ia, ib)],
            me(),
            T0,
            Some(0),
        )
        .unwrap();
        let err = repo
            .execute(live, vec![link(ib, ia)], me(), T0, None)
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Cycle);
        let err = repo
            .execute(
                live,
                vec![Command::DeleteActivity {
                    id: ia,
                    reconnect: false,
                }],
                me(),
                T0,
                Some(0),
            )
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::VersionConflict);
        assert_eq!(err.details["current"], 1);
    }

    #[test]
    fn st_02_delete_cascades_and_reconnects_deterministically() {
        let mut repo = repo();
        let live = repo.live;
        let (a, b, c) = (act(&repo, "A"), act(&repo, "B"), act(&repo, "C"));
        let (ia, ib, ic) = (a.id, b.id, c.id);
        repo.execute(
            live,
            vec![upsert(a), upsert(b), upsert(c), link(ia, ib), link(ib, ic)],
            me(),
            T0,
            None,
        )
        .unwrap();
        let mut other = repo.clone();
        let e = repo
            .execute(
                live,
                vec![Command::DeleteActivity {
                    id: ib,
                    reconnect: true,
                }],
                me(),
                T0,
                None,
            )
            .unwrap();
        let head = &repo.branch(live).unwrap().head;
        assert_eq!(head.relationships.len(), 1);
        let r = head.relationships.values().next().unwrap();
        assert_eq!((r.pred, r.succ, r.kind), (ia, ic, RelationKind::FS));
        assert_eq!(e.changes.len(), 4, "B, two removed links, one new link");
        other
            .execute(
                live,
                vec![Command::DeleteActivity {
                    id: ib,
                    reconnect: true,
                }],
                me(),
                T0,
                None,
            )
            .unwrap();
        assert_eq!(
            other.branch(live).unwrap().head,
            *head,
            "replay is reproducible"
        );
    }
}

mod branches {
    use super::*;

    #[test]
    fn st_03_04_fork_is_isolated_and_diffable() {
        let mut repo = seeded();
        let live = repo.live;
        let sb = repo.fork(live, "what-if", agent(), T0, Some(60)).unwrap();
        assert_eq!(
            repo.branch(sb).unwrap().head,
            repo.branch(live).unwrap().head
        );
        assert!(repo.diff(live, sb).unwrap().is_empty());
        let a = code(&repo, sb, "A");
        repo.execute(sb, vec![set_dur(&repo, sb, a, 7)], agent(), T0, None)
            .unwrap();
        let d = repo.diff(live, sb).unwrap();
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].fields, ["duration"]);
        assert_eq!(dur(&repo, live, a), 3, "live untouched");
    }

    /// Review H1: a project never grows past what a project file holds.
    #[test]
    fn st_03_fork_count_is_capped() {
        let mut repo = seeded();
        let live = repo.live;
        for i in 1..crate::MAX_FILE_BRANCHES {
            repo.fork(live, format!("s{i}"), agent(), T0, None).unwrap();
        }
        let err = repo.fork(live, "one more", agent(), T0, None).unwrap_err();
        assert_eq!(err.code, prswt_core::ErrorCode::Limit);
    }

    #[test]
    fn st_05_merge_without_conflict_combines_both_sides() {
        let mut repo = seeded();
        let live = repo.live;
        let sb = repo.fork(live, "s", agent(), T0, None).unwrap();
        let (a, b) = (code(&repo, live, "A"), code(&repo, live, "B"));
        repo.execute(sb, vec![set_dur(&repo, sb, a, 9)], agent(), T0, None)
            .unwrap();
        // live edits a different field of the same entity and another entity
        let mut renamed = repo.branch(live).unwrap().head.activities[&a].clone();
        renamed.name = "Excavate".into();
        repo.execute(
            live,
            vec![upsert(renamed), set_dur(&repo, live, b, 6)],
            me(),
            T0,
            None,
        )
        .unwrap();
        let (event, conflicts) = repo
            .merge(sb, MergeStrategy::RejectOnConflict, me(), T0)
            .unwrap();
        assert!(conflicts.is_empty());
        assert_eq!(event.kind, EventKind::Merge { from: sb });
        let head = &repo.branch(live).unwrap().head;
        assert_eq!(head.activities[&a].name, "Excavate");
        assert_eq!(dur(&repo, live, a), 9);
        assert_eq!(dur(&repo, live, b), 6);
        assert_eq!(repo.branch(sb).unwrap().meta.status, BranchStatus::Merged);
    }

    #[test]
    fn st_05_conflicts_follow_strategy() {
        for (strategy, want) in [
            (MergeStrategy::RejectOnConflict, None),
            (MergeStrategy::PreferSandbox, Some(9)),
            (MergeStrategy::PreferLive, Some(5)),
        ] {
            let mut repo = seeded();
            let live = repo.live;
            let sb = repo.fork(live, "s", agent(), T0, None).unwrap();
            let a = code(&repo, live, "A");
            repo.execute(sb, vec![set_dur(&repo, sb, a, 9)], agent(), T0, None)
                .unwrap();
            repo.execute(live, vec![set_dur(&repo, live, a, 5)], me(), T0, None)
                .unwrap();
            match (repo.merge(sb, strategy, me(), T0), want) {
                (Err(e), None) => {
                    assert_eq!(e.code, ErrorCode::MergeConflict);
                    assert_eq!(e.details[0]["field"], "duration");
                }
                (Ok((_, c)), Some(d)) => {
                    assert_eq!(c.len(), 1);
                    assert_eq!(dur(&repo, live, a), d, "{strategy:?}");
                }
                other => panic!("{strategy:?}: {other:?}"),
            }
        }
    }

    #[test]
    fn st_05_merge_result_must_be_valid() {
        let mut repo = seeded();
        let live = repo.live;
        let sb = repo.fork(live, "s", agent(), T0, None).unwrap();
        let (a, c) = (code(&repo, live, "A"), code(&repo, live, "C"));
        repo.execute(sb, vec![link(a, c)], agent(), T0, None)
            .unwrap();
        repo.execute(
            live,
            vec![Command::DeleteActivity {
                id: c,
                reconnect: false,
            }],
            me(),
            T0,
            None,
        )
        .unwrap();
        let err = repo
            .merge(sb, MergeStrategy::PreferSandbox, me(), T0)
            .unwrap_err();
        assert_eq!(
            err.code,
            ErrorCode::Validation,
            "link to a deleted activity"
        );
        assert_eq!(repo.branch(sb).unwrap().meta.status, BranchStatus::Open);
    }

    #[test]
    fn st_06_ttl_expiry_and_discard() {
        let mut repo = seeded();
        let live = repo.live;
        let short = repo.fork(live, "short", agent(), T0, Some(30)).unwrap();
        let keep = repo.fork(live, "keep", agent(), T0, None).unwrap();
        assert!(repo.expire(T0.plus_minutes(29)).is_empty());
        assert_eq!(repo.expire(T0.plus_minutes(30)), vec![short]);
        assert!(
            repo.execute(
                short,
                vec![link(code(&repo, live, "A"), code(&repo, live, "C"))],
                agent(),
                T0,
                None
            )
            .is_err()
        );
        repo.discard(keep).unwrap();
        assert_eq!(
            repo.branch(keep).unwrap().meta.status,
            BranchStatus::Discarded
        );
        assert!(repo.discard(live).is_err());
    }

    #[test]
    fn st_08_undo_redo() {
        let mut repo = seeded();
        let live = repo.live;
        let a = code(&repo, live, "A");
        let before = repo.branch(live).unwrap().head.clone();
        repo.execute(live, vec![set_dur(&repo, live, a, 8)], me(), T0, None)
            .unwrap();
        repo.undo(live, me(), T0).unwrap();
        assert_eq!(repo.branch(live).unwrap().head, before);
        repo.redo(live, me(), T0).unwrap();
        assert_eq!(dur(&repo, live, a), 8);
        assert!(repo.redo(live, me(), T0).is_err(), "redo stack empty");
        assert_eq!(repo.replay(live).unwrap(), repo.branch(live).unwrap().head);
    }
}

// ───────────────────────────── property tests ─────────────────────────────

mod files {
    use super::*;

    #[test]
    fn dsk_01_file_round_trips_heads_sandboxes_and_history() {
        let mut repo = seeded();
        let live = repo.live;
        let sb = repo.fork(live, "what-if", agent(), T0, Some(60)).unwrap();
        let a = code(&repo, sb, "A");
        repo.execute(sb, vec![set_dur(&repo, sb, a, 7)], agent(), T0, None)
            .unwrap();
        let text = serde_json::to_string(&repo.to_file()).unwrap();
        let back = Repo::from_file(serde_json::from_str(&text).unwrap()).unwrap();
        assert_eq!(back.project, repo.project);
        assert_eq!(back.live, live);
        for b in repo.branches() {
            let r = back.branch(b.meta.id).unwrap();
            assert_eq!(r.meta, b.meta);
            assert_eq!(r.head, b.head);
            assert_eq!(r.events, b.events);
        }
        assert_eq!(dur(&back, sb, a), 7);
        assert_eq!(dur(&back, live, a), 3);
        for b in repo.branches() {
            let r = back.branch(b.meta.id).unwrap();
            assert_eq!(r.history_depth(), b.history_depth(), "gap 9: history kept");
        }
        let mut back = back;
        back.undo(sb, agent(), T0).unwrap();
        assert_eq!(dur(&back, sb, a), 3, "undo works after reopening");
        back.redo(sb, agent(), T0).unwrap();
        assert_eq!(dur(&back, sb, a), 7);
    }

    #[test]
    fn dsk_01_hostile_files_are_rejected() {
        let repo = seeded();
        let live = repo.live;
        let good = repo.to_file();
        let reject = |f: ProjectFile, why: &str| {
            let e = Repo::from_file(f).unwrap_err();
            assert_eq!(e.code, ErrorCode::Validation, "{why}: {e}");
        };
        let mut f = good.clone();
        f.format = "other".into();
        reject(f, "format");
        let mut f = good.clone();
        f.version = FILE_VERSION + 1;
        reject(f, "version");
        let mut f = good.clone();
        f.branches.clear();
        reject(f, "no branches");
        let mut f = good.clone();
        f.branches.push(f.branches[0].clone());
        reject(f, "duplicate branch");
        let mut f = good.clone();
        f.live = prswt_core::ids::BranchId::new();
        reject(f, "live missing");
        let mut f = good.clone();
        f.project = ProjectId::new();
        reject(f, "project id");
        let mut f = good.clone();
        f.branches[0].meta.version += 1;
        reject(f, "version mismatch");
        let mut f = good.clone();
        f.branches[0].events[0].version = 2;
        reject(f, "event order");
        let mut f = good.clone();
        f.branches[0].events[0].branch = prswt_core::ids::BranchId::new();
        reject(f, "foreign event");
        let mut f = good.clone();
        f.branches[0].base.activities.clear();
        f.branches[0].base.wbs.clear();
        reject(f, "base fails integrity");
        let mut f = good.clone();
        f.branches[0].undo = vec![999];
        reject(f, "undo names a missing event");
        let mut f = good.clone();
        f.branches[0].undo = vec![1, 1];
        reject(f, "undo repeats an event");
        assert!(Repo::from_file(good).is_ok_and(|r| r.live == live));
    }
}

#[derive(Debug, Clone)]
enum Op {
    Add(u8),
    Del(u8, bool),
    Link(u8, u8),
    Dur(u8, u8),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0u8..12).prop_map(Op::Add),
        ((0u8..12), any::<bool>()).prop_map(|(a, r)| Op::Del(a, r)),
        ((0u8..12), (0u8..12)).prop_map(|(a, b)| Op::Link(a, b)),
        ((0u8..12), (1u8..20)).prop_map(|(a, d)| Op::Dur(a, d)),
    ]
}

fn to_command(repo: &Repo, b: prswt_core::ids::BranchId, op: &Op) -> Option<Command> {
    let head = &repo.branch(b).ok()?.head;
    let nth = |k: u8| {
        head.activities
            .values()
            .nth(usize::from(k) % head.activities.len().max(1))
            .map(|a| a.id)
    };
    match op {
        Op::Add(k) => Some(upsert(act(repo, &format!("N{k}")))),
        Op::Del(k, r) => nth(*k).map(|id| Command::DeleteActivity { id, reconnect: *r }),
        Op::Link(x, y) => Some(link(nth(*x)?, nth(*y)?)),
        Op::Dur(k, d) => {
            let mut a = head.activities.get(&nth(*k)?)?.clone();
            a.duration = Estimate::fixed(WorkMinutes(i64::from(*d) * 480));
            Some(upsert(a))
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    /// Invariants 4–6 and ST-08 over random command sequences.
    #[test]
    fn st_07_invariants(ops in proptest::collection::vec(op(), 1..40), sandbox_ops in proptest::collection::vec(op(), 1..15)) {
        let mut repo = repo();
        let live = repo.live;
        let initial = repo.branch(live).unwrap().head.clone();
        let mut applied = 0usize;
        for o in &ops {
            if let Some(c) = to_command(&repo, live, o)
                && repo.execute(live, vec![c], me(), T0, None).is_ok()
            {
                applied += 1;
            }
        }
        // 4: event replay reproduces head
        prop_assert_eq!(repo.replay(live).unwrap(), repo.branch(live).unwrap().head.clone());

        // 5: a fresh fork equals its parent
        let sb = repo.fork(live, "p", agent(), T0, None).unwrap();
        prop_assert!(repo.diff(live, sb).unwrap().is_empty());

        // 6: merging a sandbox into an untouched parent yields the sandbox head
        for o in &sandbox_ops {
            if let Some(c) = to_command(&repo, sb, o) {
                let _ = repo.execute(sb, vec![c], agent(), T0, None);
            }
        }
        let sandbox_head = repo.branch(sb).unwrap().head.clone();
        repo.merge(sb, MergeStrategy::RejectOnConflict, me(), T0).unwrap();
        prop_assert_eq!(repo.branch(live).unwrap().head.clone(), sandbox_head);

        // ST-08: undoing everything (merge + commands, within depth) restores the start
        repo.undo(live, me(), T0).unwrap();
        for _ in 0..applied.min(UNDO_DEPTH - 1) {
            repo.undo(live, me(), T0).unwrap();
        }
        if applied < UNDO_DEPTH {
            prop_assert_eq!(repo.branch(live).unwrap().head.clone(), initial);
        }
    }
}

// ─────────────────────────────── helpers ───────────────────────────────

fn me() -> Author {
    Author::Human("planner".into())
}

fn agent() -> Author {
    Author::Agent("auditor".into())
}

fn repo() -> Repo {
    let cal = CalendarDef {
        id: CalendarId::new(),
        name: "5x8".into(),
        base: None,
        week: Some(WorkWeek::standard_5x8()),
        exceptions: BTreeMap::new(),
        minutes_per_day: 480,
    };
    let project = Project {
        id: ProjectId::new(),
        portfolio: None,
        name: "P".into(),
        data_date: T0,
        planned_start: T0,
        must_finish_by: None,
        default_calendar: cal.id,
        settings: ScheduleSettings::default(),
    };
    let root = WbsNode {
        id: WbsId::new(),
        parent: None,
        code: "1".into(),
        name: "P".into(),
        obs: None,
        sort_key: 0,
    };
    Repo::new(ProjectState::new(project, cal, root), Author::System, T0).unwrap()
}

/// A(3) → B(3), C(3) standalone.
fn seeded() -> Repo {
    let mut r = repo();
    let live = r.live;
    let (a, b, c) = (act(&r, "A"), act(&r, "B"), act(&r, "C"));
    let (ia, ib) = (a.id, b.id);
    r.execute(
        live,
        vec![upsert(a), upsert(b), upsert(c), link(ia, ib)],
        me(),
        T0,
        None,
    )
    .unwrap();
    r
}

fn act(repo: &Repo, code: &str) -> Activity {
    let head = &repo.branch(repo.live).unwrap().head;
    Activity {
        id: ActivityId::new(),
        code: code.into(),
        name: code.into(),
        wbs: *head.wbs.keys().next().unwrap(),
        kind: ActivityKind::TaskDependent,
        calendar: head.project.default_calendar,
        duration: Estimate::fixed(WorkMinutes(3 * 480)),
        remaining: None,
        primary_constraint: None,
        secondary_constraint: None,
        actual_start: None,
        actual_finish: None,
        pct_complete: Decimal::ZERO,
        pct_type: PercentCompleteType::Duration,
        expected_finish: None,
        suspend: None,
        resume: None,
        leveled_start: None,
    }
}

fn upsert(a: Activity) -> Command {
    Command::UpsertActivity {
        activity: Box::new(a),
    }
}

fn link(pred: ActivityId, succ: ActivityId) -> Command {
    Command::UpsertRelationship {
        relationship: Relationship {
            id: RelationshipId::new(),
            pred,
            succ,
            kind: RelationKind::FS,
            lag: WorkMinutes(0),
            lag_calendar: None,
        },
    }
}

fn code(repo: &Repo, b: prswt_core::ids::BranchId, c: &str) -> ActivityId {
    repo.branch(b)
        .unwrap()
        .head
        .activities
        .values()
        .find(|a| a.code == c)
        .unwrap()
        .id
}

fn set_dur(repo: &Repo, b: prswt_core::ids::BranchId, id: ActivityId, days: i64) -> Command {
    let mut a = repo.branch(b).unwrap().head.activities[&id].clone();
    a.duration = Estimate::fixed(WorkMinutes(days * 480));
    upsert(a)
}

fn dur(repo: &Repo, b: prswt_core::ids::BranchId, id: ActivityId) -> i64 {
    repo.branch(b).unwrap().head.activities[&id]
        .duration
        .planning_value()
        .0
        / 480
}
