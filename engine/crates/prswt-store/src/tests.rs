//! Database tests. They need `PRSWT_TEST_DATABASE_URL` (the gate sets it to
//! the `prswt_test` database in the local container) and are skipped
//! without it. The guard refuses any database whose name lacks "test".

use std::collections::BTreeMap;

use prswt_core::ids::{ActivityId, CalendarId, RelationshipId, WbsId};
use prswt_core::model::{
    Activity, ActivityKind, CalendarDef, Estimate, PercentCompleteType, Project, RelationKind,
    Relationship, ScheduleSettings, WbsNode, WorkWeek,
};
use prswt_core::{Instant, WorkMinutes};
use prswt_state::{Author, Command, MergeStrategy};
use rust_decimal::Decimal;

use super::*;

const T0: Instant = Instant(14_000_000);

#[test]
fn db_04_roundtrip_all_branches_and_incremental_sync() {
    let Some(store) = store() else { return };
    let mut repo = repo();
    let _cleanup = Cleanup(&store, repo.project);
    let live = repo.live;
    let a = add(&mut repo, live, "A", 3);
    let b = add(&mut repo, live, "B", 4);
    link(&mut repo, live, a, b);
    let sb = repo
        .fork(live, "what-if", Author::Agent("t".into()), T0, Some(60))
        .unwrap();
    add(&mut repo, sb, "C", 2);
    let sb2 = repo
        .fork(live, "merged", Author::Agent("t".into()), T0, None)
        .unwrap();
    add(&mut repo, sb2, "D", 1);
    repo.merge(
        sb2,
        MergeStrategy::RejectOnConflict,
        Author::Human("h".into()),
        T0,
    )
    .unwrap();
    repo.undo(live, Author::Human("h".into()), T0).unwrap();

    let first = store.sync(&repo).unwrap();
    assert_eq!(first.branches, 3);
    assert_eq!(
        first.events, 7,
        "live 3 + merge + undo, what-if 1, merged 1"
    );
    let loaded = store.load(repo.project).unwrap();
    for br in repo.branches() {
        let got = loaded.branch(br.meta.id).unwrap();
        assert_eq!(got.head, br.head, "{}", br.meta.name);
        assert_eq!(got.meta, br.meta, "{}", br.meta.name);
        assert_eq!(got.events, br.events);
    }
    assert_eq!(
        loaded.replay(live).unwrap(),
        repo.branch(live).unwrap().head
    );

    add(&mut repo, live, "E", 5);
    let second = store.sync(&repo).unwrap();
    assert_eq!(second.events, 1, "only the new event is written");
    assert_eq!(store.sync(&repo).unwrap().events, 0, "idempotent");
    store.delete_project(repo.project).unwrap();
}

#[test]
fn db_03_concurrent_writer_is_detected() {
    let Some(store) = store() else { return };
    let mut a = repo();
    let _cleanup = Cleanup(&store, a.project);
    let live = a.live;
    add(&mut a, live, "A", 1);
    store.sync(&a).unwrap();
    let mut b = store.load(a.project).unwrap();
    add(&mut b, live, "B", 1);
    store.sync(&b).unwrap();
    add(&mut a, live, "X", 1); // same version number as b's event
    // a is at version 2 in memory, the database too, but with b's event.
    let err = store.sync(&a).unwrap_err();
    assert_eq!(err.code, ErrorCode::VersionConflict);
    add(&mut b, live, "C", 1);
    store.sync(&b).unwrap();
    store.delete_project(a.project).unwrap();
}

#[test]
fn db_04_snapshots_written_and_verified() {
    let Some(store) = store() else { return };
    let mut repo = repo();
    let _cleanup = Cleanup(&store, repo.project);
    let live = repo.live;
    for i in 0..105 {
        add(&mut repo, live, &format!("N{i}"), 1);
    }
    let stats = store.sync(&repo).unwrap();
    assert_eq!((stats.events, stats.snapshots), (105, 1));
    let loaded = store.load(repo.project).unwrap();
    assert_eq!(loaded.branch(live).unwrap().head.activities.len(), 105);
    store.delete_project(repo.project).unwrap();
}

#[test]
fn auth_02_03_sessions_and_tokens() {
    let Some(store) = store() else { return };
    let email = format!("t-{}@example.test", ProjectId::new());
    let s_hash = format!("s-{}", ProjectId::new());
    store.create_session(&s_hash, &email, "google", 1).unwrap();
    assert_eq!(
        store.session_email(&s_hash).unwrap().as_deref(),
        Some(email.as_str())
    );
    store.delete_session(&s_hash).unwrap();
    assert_eq!(store.session_email(&s_hash).unwrap(), None, "logged out");
    store.create_session(&s_hash, &email, "google", 0).unwrap();
    assert_eq!(
        store.session_email(&s_hash).unwrap(),
        None,
        "expired immediately"
    );

    let id = uuid::Uuid::now_v7();
    let t_hash = format!("t-{id}");
    store
        .create_token(id, &t_hash, &email, "laptop", "agent")
        .unwrap();
    let owner = store.use_token(&t_hash).unwrap().unwrap();
    assert_eq!(
        (owner.email.as_str(), owner.name.as_str()),
        (email.as_str(), "laptop")
    );
    let list = store.list_tokens(&email).unwrap();
    assert_eq!(list.len(), 1);
    assert!(list[0].last_used_at.is_some());
    assert!(
        !store.revoke_token(id, "someone@else.test").unwrap(),
        "only the owner revokes"
    );
    assert!(store.revoke_token(id, &email).unwrap());
    assert_eq!(
        store.use_token(&t_hash).unwrap(),
        None,
        "revoked tokens stop working"
    );
}

// ─────────────────────────────── helpers ───────────────────────────────

/// Deletes the test project when dropped, so a failing assertion cannot
/// leave rows behind in the test database.
#[test]
fn sync_01_members_roles_and_settings() {
    let Some(store) = store() else { return };
    let p = uuid::Uuid::now_v7();
    let (own, ed) = (format!("own-{p}@t.test"), format!("ed-{p}@t.test"));
    store.set_member(p, &own, "owner", "test").unwrap();
    store.set_member(p, &ed, "editor", &own).unwrap();
    assert!(
        store.set_member(p, &ed, "boss", &own).is_err(),
        "unknown role"
    );
    assert_eq!(
        store.access_for(&ed).unwrap(),
        vec![(p, "editor".to_owned())]
    );
    assert!(store.is_member_anywhere(&ed).unwrap());
    let m = store.members(p).unwrap();
    assert_eq!(
        (m[0].email.as_str(), m[0].role.as_str()),
        (own.as_str(), "owner")
    );
    // The last owner can be neither demoted nor removed.
    assert!(store.set_member(p, &own, "viewer", &own).is_err());
    assert!(store.remove_member(p, &own).is_err());
    assert!(store.remove_member(p, &ed).unwrap());
    assert!(!store.is_member_anywhere(&ed).unwrap());
    store
        .put_settings(&own, &serde_json::json!({ "theme": "dark" }))
        .unwrap();
    assert_eq!(store.settings(&own).unwrap().unwrap()["theme"], "dark");
    assert!(store.put_settings(&own, &serde_json::json!([1])).is_err());
    store.delete_project(ProjectId(p)).unwrap();
    assert!(store.members(p).unwrap().is_empty());
}

/// Review M1/M2: a stored project is never claimed by a new owner, and a
/// project without members is not adopted again on a later start.
#[test]
fn sync_06_claims_and_one_time_adoption() {
    let Some(store) = store() else { return };
    store.adopt_orphans(&[]).unwrap(); // the one-time run, if not done yet
    let r = repo();
    let _cleanup = Cleanup(&store, r.project);
    store.sync(&r).unwrap();
    let who = format!("late-{}@t.test", r.project);
    assert_eq!(store.adopt_orphans(std::slice::from_ref(&who)).unwrap(), 0);
    assert!(
        store.members(r.project.0).unwrap().is_empty(),
        "not adopted on restart"
    );
    assert!(
        store.claim_project(r.project.0, &who).is_err(),
        "a stored project cannot be claimed"
    );
    let fresh = uuid::Uuid::now_v7();
    store.claim_project(fresh, &who).unwrap();
    assert!(
        store.claim_project(fresh, "other@t.test").is_err(),
        "claimed once"
    );
    // Its only member, the owner, cannot leave it ownerless.
    assert!(store.remove_member(fresh, &who).is_err());
    store.delete_project(ProjectId(fresh)).unwrap();
}

struct Cleanup<'a>(&'a Store, ProjectId);

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        let _ = self.0.delete_project(self.1);
    }
}

fn store() -> Option<Store> {
    let url = std::env::var("PRSWT_TEST_DATABASE_URL").ok()?;
    let db = url.rsplit('/').next().unwrap_or_default();
    assert!(
        db.contains("test"),
        "refusing to run tests against database `{db}`"
    );
    Some(Store::connect(&url).unwrap())
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
        name: format!("store-test-{}", ProjectId::new()),
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

fn add(repo: &mut Repo, b: BranchId, code: &str, days: i64) -> ActivityId {
    let head = &repo.branch(b).unwrap().head;
    let a = Activity {
        id: ActivityId::new(),
        code: code.into(),
        name: code.into(),
        wbs: *head.wbs.keys().next().unwrap(),
        kind: ActivityKind::TaskDependent,
        calendar: head.project.default_calendar,
        duration: Estimate::fixed(WorkMinutes(days * 480)),
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
    };
    let id = a.id;
    repo.execute(
        b,
        vec![Command::UpsertActivity {
            activity: Box::new(a),
        }],
        Author::Human("h".into()),
        T0,
        None,
    )
    .unwrap();
    id
}

fn link(repo: &mut Repo, b: BranchId, pred: ActivityId, succ: ActivityId) {
    let r = Relationship {
        id: RelationshipId::new(),
        pred,
        succ,
        kind: RelationKind::FS,
        lag: WorkMinutes(0),
        lag_calendar: None,
    };
    repo.execute(
        b,
        vec![Command::UpsertRelationship { relationship: r }],
        Author::Human("h".into()),
        T0,
        None,
    )
    .unwrap();
}
