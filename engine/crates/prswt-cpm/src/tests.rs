//! Oracle: `vault/05-Math/CPM-oracles.md`.
#![allow(clippy::unused_self)]

use std::collections::BTreeMap;

use proptest::prelude::*;
use prswt_core::ids::{ActivityId, CalendarId, ProjectId, RelationshipId, WbsId};
use prswt_core::model::{
    Activity, ActivityKind, CalendarDef, Constraint, ConstraintKind, CriticalDefinition, Estimate,
    LagCalendar, PercentCompleteType, ProgressMode, Project, RelationKind, Relationship,
    ScheduleSettings, WbsNode, WorkWeek,
};
use prswt_core::{ErrorCode, Instant, WorkMinutes};
use rust_decimal::Decimal;

use super::*;

const DAY: i64 = 1440;

mod oracle_n1 {
    use super::*;

    #[test]
    fn cpm_03_06_10_baseline_network() {
        let net = n1();
        let s = net.run();
        let expect = [
            ("A", 0, 3, 0, 3, 0, 0, true),
            ("B", 3, 7, 3, 7, 0, 0, true),
            ("C", 3, 5, 3, 5, 0, 0, true),
            ("D", 7, 12, 7, 12, 0, 0, true),
            ("E", 4, 7, 10, 13, 6, 6, false),
            ("F", 12, 14, 12, 14, 0, 0, true),
        ];
        for (code, es, ef, ls, lf, tf, ff, crit) in expect {
            let a = net.get(&s, code);
            assert_eq!(
                (
                    net.day(a.early_start),
                    net.day(a.early_finish),
                    net.day(a.late_start),
                    net.day(a.late_finish)
                ),
                (es, ef, ls, lf),
                "{code} dates"
            );
            assert_eq!(a.total_float, Some(WorkMinutes(tf * DAY)), "{code} TF");
            assert_eq!(a.free_float, Some(WorkMinutes(ff * DAY)), "{code} FF");
            assert_eq!(a.critical, crit, "{code} critical");
        }
        assert_eq!(net.day(s.project_finish()), 14);
    }

    #[test]
    fn cpm_07_snet_constraint() {
        let mut net = n1();
        net.constrain("C", ConstraintKind::StartNoEarlierThan, 5);
        let s = net.run();
        let row = |c| {
            let a = net.get(&s, c);
            (
                net.day(a.early_start),
                net.day(a.early_finish),
                a.total_float.map(|t| t.0 / DAY),
            )
        };
        assert_eq!(row("C"), (5, 7, Some(0)));
        assert_eq!(row("D"), (9, 14, Some(0)));
        assert_eq!(row("E"), (6, 9, Some(6)));
        assert_eq!(row("F"), (14, 16, Some(0)));
        assert_eq!(row("A"), (0, 3, Some(2)));
        assert_eq!(row("B"), (3, 7, Some(2)));
        assert_eq!(net.get(&s, "A").free_float, Some(WorkMinutes(0)));
        assert!(matches!(net.get(&s, "C").driver, Driver::Constraint));
    }

    #[test]
    fn cpm_08_must_finish_by_negative_float() {
        let mut net = n1();
        net.project.must_finish_by = Some(net.at(12));
        let s = net.run();
        for c in ["A", "B", "C", "D", "F"] {
            assert_eq!(
                net.get(&s, c).total_float,
                Some(WorkMinutes(-2 * DAY)),
                "{c}"
            );
        }
        assert_eq!(net.get(&s, "E").total_float, Some(WorkMinutes(4 * DAY)));
        assert_eq!(net.day(net.get(&s, "A").late_start), -2);
    }

    #[test]
    fn cpm_08_mandatory_finish() {
        let mut net = n1();
        net.constrain("F", ConstraintKind::MandatoryFinish, 15);
        let s = net.run();
        let f = net.get(&s, "F");
        assert_eq!((net.day(f.early_start), net.day(f.early_finish)), (13, 15));
        assert_eq!(f.total_float, Some(WorkMinutes(0)));
        for c in ["A", "B", "C", "D"] {
            assert_eq!(net.get(&s, c).total_float, Some(WorkMinutes(DAY)), "{c}");
        }
        assert_eq!(net.get(&s, "E").total_float, Some(WorkMinutes(7 * DAY)));
        assert_eq!(net.day(s.project_finish()), 15);
    }

    #[test]
    fn cpm_04_sf_link() {
        let mut net = n1();
        net.add("G", 1, ActivityKind::TaskDependent);
        net.link("A", "G", RelationKind::SF, 2 * DAY);
        let s = net.run();
        let g = net.get(&s, "G");
        assert_eq!((net.day(g.early_start), net.day(g.early_finish)), (1, 2));
        assert_eq!(g.total_float, Some(WorkMinutes(12 * DAY)));
    }

    #[test]
    fn cpm_09_alap_consumes_free_float() {
        let mut net = n1();
        net.constrain_no_date("E", ConstraintKind::AsLateAsPossible);
        let s = net.run();
        let e = net.get(&s, "E");
        assert_eq!((net.day(e.early_start), net.day(e.early_finish)), (10, 13));
        assert_eq!(e.free_float, Some(WorkMinutes(0)));
        assert_eq!(e.total_float, Some(WorkMinutes(0)));
    }

    #[test]
    fn cpm_09_alap_chain_slides_together() {
        // M (ALAP milestone) -> B (ALAP 3d) -> C; X (10d) also drives C.
        // B slides to finish when C starts (day 10), then M follows B.
        let mut net = Net::new(WorkWeek::continuous(), DAY);
        net.add("M", 0, ActivityKind::StartMilestone);
        for (c, d) in [("B", 3), ("C", 2), ("X", 10)] {
            net.add(c, d, ActivityKind::TaskDependent);
        }
        net.link("M", "B", RelationKind::FS, 0);
        net.link("B", "C", RelationKind::FS, 0);
        net.link("X", "C", RelationKind::FS, 0);
        net.constrain_no_date("M", ConstraintKind::AsLateAsPossible);
        net.constrain_no_date("B", ConstraintKind::AsLateAsPossible);
        let s = net.run();
        let (m, b) = (net.get(&s, "M"), net.get(&s, "B"));
        assert_eq!((net.day(b.early_start), net.day(b.early_finish)), (7, 10));
        assert_eq!(net.day(m.early_start), 7);
        assert_eq!(b.free_float, Some(WorkMinutes(0)));
        assert_eq!(b.total_float, Some(WorkMinutes(0)));
        assert_eq!(m.total_float, Some(WorkMinutes(0)));
    }

    #[test]
    fn cpm_14_longest_path() {
        let mut net = n1();
        net.project.must_finish_by = Some(net.at(20));
        net.project.settings.critical = CriticalDefinition::LongestPath;
        let s = net.run();
        let crit: Vec<&str> = ["A", "B", "C", "D", "E", "F"]
            .into_iter()
            .filter(|c| net.get(&s, c).critical)
            .collect();
        assert_eq!(crit, ["A", "B", "C", "D", "F"]);
        assert!(net.get(&s, "E").total_float > Some(WorkMinutes(0)));
    }

    #[test]
    fn cpm_15_float_paths() {
        let net = n1();
        let s = net.run();
        let paths: Vec<(Vec<String>, i64)> = s
            .float_paths(5)
            .into_iter()
            .map(|p| {
                let codes = p
                    .activities
                    .iter()
                    .map(|&i| s.model.nodes[i].code.clone())
                    .collect();
                (codes, p.float / DAY)
            })
            .collect();
        assert_eq!(
            paths,
            vec![
                (vec!["A".into(), "B".into(), "D".into(), "F".into()], 0),
                (vec!["C".into()], 0),
                (vec!["E".into()], 6),
            ]
        );
    }

    #[test]
    fn cpm_16_explain_driver_chain() {
        let net = n1();
        let s = net.run();
        let f = s.index_of_code("F").unwrap();
        let chain: Vec<String> = s
            .explain(f)
            .iter()
            .map(|st| s.model.nodes[st.activity].code.clone())
            .collect();
        assert_eq!(chain, ["F", "D", "B", "A"]);
        assert_eq!(
            s.explain(s.index_of_code("A").unwrap())[0].driver,
            Driver::ProjectStart
        );
    }

    #[test]
    fn cpm_17_dcma_on_n1() {
        let net = n1();
        let s = net.run();
        let report = dcma::assess(&s, &dcma::DcmaContext::default());
        let get = |n: u8| report.checks.iter().find(|c| c.number == n).unwrap();
        assert_eq!(report.checks.len(), 14);
        assert_eq!(
            get(1).pass,
            Some(false),
            "A has no predecessor, F no successor"
        );
        assert_eq!(get(1).offenders, ["A", "F"]);
        assert_eq!(get(2).pass, Some(true));
        assert_eq!(get(3).pass, Some(false), "2 of 7 links lagged");
        assert_eq!(get(4).pass, Some(false), "5/7 FS");
        assert_eq!(get(7).pass, Some(true));
        assert_eq!(get(11).pass, None, "no baseline");
        assert_eq!(get(12).pass, Some(true), "critical path test");
        assert!(report.score > 0.0 && report.score < 1.0);
    }
}

mod calendars_n2 {
    use super::*;

    #[test]
    fn cpm_11_fs_across_weekend_and_lag_calendars() {
        let base = |lag: i64, policy: Option<LagCalendar>| {
            let mut net = Net::new(WorkWeek::standard_5x8(), 480);
            net.project.planned_start = Instant::from_ymd_hm(2026, 10, 23, 8, 0).unwrap();
            net.project.data_date = net.project.planned_start;
            net.add_minutes("A", 480, ActivityKind::TaskDependent);
            net.add_minutes("B", 480, ActivityKind::TaskDependent);
            net.link("A", "B", RelationKind::FS, lag);
            if let Some(p) = policy {
                net.rels[0].lag_calendar = Some(p);
            }
            let s = net.run();
            let b = net.get(&s, "B");
            (b.early_start.to_string(), b.early_finish.to_string())
        };
        assert_eq!(
            base(0, None),
            ("2026-10-26T08:00".into(), "2026-10-26T17:00".into())
        );
        assert_eq!(
            base(240, None),
            ("2026-10-26T13:00".into(), "2026-10-27T12:00".into())
        );
        assert_eq!(
            base(240, Some(LagCalendar::TwentyFourHour)),
            ("2026-10-26T08:00".into(), "2026-10-26T17:00".into())
        );
    }

    #[test]
    fn cpm_12_milestones_snap_by_type() {
        let mut net = Net::new(WorkWeek::standard_5x8(), 480);
        net.project.planned_start = Instant::from_ymd_hm(2026, 10, 23, 8, 0).unwrap();
        net.project.data_date = net.project.planned_start;
        net.add_minutes("A", 480, ActivityKind::TaskDependent);
        net.add_minutes("FM", 0, ActivityKind::FinishMilestone);
        net.add_minutes("SM", 0, ActivityKind::StartMilestone);
        net.link("A", "FM", RelationKind::FS, 0);
        net.link("A", "SM", RelationKind::FS, 0);
        let s = net.run();
        assert_eq!(
            net.get(&s, "FM").early_finish.to_string(),
            "2026-10-23T17:00"
        );
        assert_eq!(
            net.get(&s, "SM").early_start.to_string(),
            "2026-10-26T08:00"
        );
        assert_eq!(net.get(&s, "SM").total_float, Some(WorkMinutes(0)));
    }

    #[test]
    fn cpm_11_mixed_calendar_free_float_on_predecessor_calendar() {
        // 24h predecessor finishing Fri 21:00 feeding a 5x8 successor that
        // starts Mon 08:00 can slip 59h on its own calendar.
        let mut net = Net::new(WorkWeek::standard_5x8(), 480);
        net.project.planned_start = Instant::from_ymd_hm(2026, 10, 23, 8, 0).unwrap();
        net.project.data_date = net.project.planned_start;
        let c24 = net.add_calendar(WorkWeek::continuous());
        net.add_minutes("P", 13 * 60, ActivityKind::TaskDependent);
        net.acts[0].calendar = c24;
        net.add_minutes("S", 480, ActivityKind::TaskDependent);
        net.link("P", "S", RelationKind::FS, 0);
        let s = net.run();
        let p = net.get(&s, "P");
        assert_eq!(p.early_finish.to_string(), "2026-10-23T21:00");
        assert_eq!(p.free_float, Some(WorkMinutes(59 * 60)));
        assert_eq!(net.get(&s, "S").early_start.to_string(), "2026-10-26T08:00");
    }
}

mod progress_and_kinds {
    use super::*;

    #[test]
    fn cpm_13_data_date_and_actuals() {
        let mut net = n1();
        net.project.data_date = net.at(5);
        net.actual("A", Some(0), Some(3));
        net.actual("B", Some(3), None);
        net.acts[1].remaining = Some(WorkMinutes(2 * DAY));
        let s = net.run();
        let d = |c| {
            let a = net.get(&s, c);
            (net.day(a.early_start), net.day(a.early_finish))
        };
        assert_eq!(d("A"), (0, 3));
        assert_eq!(d("B"), (3, 7), "remaining 2d from data date 5");
        assert_eq!(d("C"), (5, 7), "not started: not before data date");
        assert_eq!(d("D"), (9, 14));
        assert_eq!(net.get(&s, "A").total_float, None, "completed: no float");
    }

    #[test]
    fn cpm_13_retained_logic_vs_progress_override() {
        let make = |mode| {
            let mut net = Net::new(WorkWeek::continuous(), DAY);
            net.project.data_date = net.at(2);
            net.project.settings.progress_mode = mode;
            net.add("X", 4, ActivityKind::TaskDependent);
            net.add("B", 3, ActivityKind::TaskDependent);
            net.link("X", "B", RelationKind::FS, 0);
            net.actual("B", Some(1), None);
            net.acts[1].remaining = Some(WorkMinutes(2 * DAY));
            let s = net.run();
            net.day(net.get(&s, "B").early_finish)
        };
        assert_eq!(
            make(ProgressMode::RetainedLogic),
            8,
            "waits for X (2..6) then 2d"
        );
        assert_eq!(
            make(ProgressMode::ProgressOverride),
            4,
            "continues from data date"
        );
    }

    #[test]
    fn cpm_13_pct_complete_derives_remaining() {
        let mut net = Net::new(WorkWeek::continuous(), DAY);
        net.project.data_date = net.at(2);
        net.add("A", 10, ActivityKind::TaskDependent);
        net.actual("A", Some(0), None);
        net.acts[0].pct_complete = Decimal::new(40, 0);
        let s = net.run();
        assert_eq!(
            net.day(net.get(&s, "A").early_finish),
            8,
            "60% of 10d left from day 2"
        );
    }

    /// Oracle (24h days): A 10d started day 0, data date 2, 8d left ⇒ EF 10.
    /// Expected finish day 6 ⇒ remaining 4d, EF 6, B (2d, FS) 6..8, A TF 0.
    /// Not-started C 5d with expected finish day 3 ⇒ data date 2..3 (1d left).
    #[test]
    fn cpm_19_expected_finish_sets_remaining() {
        let make = |use_it| {
            let mut net = Net::new(WorkWeek::continuous(), DAY);
            net.project.data_date = net.at(2);
            net.project.settings.use_expected_finish = use_it;
            net.add("A", 10, ActivityKind::TaskDependent);
            net.add("B", 2, ActivityKind::TaskDependent);
            net.add("C", 5, ActivityKind::TaskDependent);
            net.link("A", "B", RelationKind::FS, 0);
            net.actual("A", Some(0), None);
            net.acts[0].pct_complete = Decimal::new(20, 0);
            let (a6, c3) = (net.at(6), net.at(3));
            net.act_mut("A").expected_finish = Some(a6);
            net.act_mut("C").expected_finish = Some(c3);
            let s = net.run();
            let d = |c| {
                let a = net.get(&s, c);
                (net.day(a.early_start), net.day(a.early_finish))
            };
            (d("A").1, d("B"), d("C"), net.get(&s, "A").total_float)
        };
        let (a, b, c, tf) = make(true);
        assert_eq!((a, b, c), (6, (6, 8), (2, 3)));
        assert_eq!(tf, Some(WorkMinutes(0)));
        let (a, b, c, _) = make(false);
        assert_eq!(
            (a, b, c),
            (10, (10, 12), (2, 7)),
            "option off: durations rule"
        );
    }

    /// A 10d, 40 % done (6d left), data date 2. Suspended at 2 and resumed
    /// at 5 ⇒ EF 11; suspended without a resume date ⇒ continues at the data date, EF 8.
    #[test]
    fn cpm_19_suspend_and_resume() {
        let make = |resume: Option<i64>| {
            let mut net = Net::new(WorkWeek::continuous(), DAY);
            net.project.data_date = net.at(2);
            net.add("A", 10, ActivityKind::TaskDependent);
            net.actual("A", Some(0), None);
            net.acts[0].pct_complete = Decimal::new(40, 0);
            net.acts[0].suspend = Some(net.at(2));
            net.acts[0].resume = resume.map(|d| net.at(d));
            let s = net.run();
            net.day(net.get(&s, "A").early_finish)
        };
        assert_eq!(make(Some(5)), 11);
        assert_eq!(make(None), 8);
    }

    #[test]
    fn cpm_19_out_of_sequence_links() {
        let mut net = Net::new(WorkWeek::continuous(), DAY);
        net.project.data_date = net.at(3);
        for c in ["X", "Y", "Z", "B"] {
            net.add(c, 4, ActivityKind::TaskDependent);
        }
        net.link("X", "B", RelationKind::FS, 0); // X not finished, B started: broken
        net.link("Y", "B", RelationKind::SS, 0); // Y started after B: broken
        net.link("Z", "B", RelationKind::FS, 0); // Z finished before B started: fine
        net.actual("Z", Some(0), Some(1));
        net.actual("B", Some(1), None);
        net.actual("Y", Some(2), None);
        let s = net.run();
        let oos: Vec<(String, RelationKind)> = s
            .out_of_sequence()
            .iter()
            .map(|o| (s.model.nodes[o.pred].code.clone(), o.kind))
            .collect();
        assert_eq!(
            oos,
            vec![
                ("X".into(), RelationKind::FS),
                ("Y".into(), RelationKind::SS)
            ]
        );
    }

    #[test]
    fn cpm_12_level_of_effort_spans_without_driving() {
        let mut net = Net::new(WorkWeek::continuous(), DAY);
        net.add("A", 3, ActivityKind::TaskDependent);
        net.add("B", 4, ActivityKind::TaskDependent);
        net.add("L", 0, ActivityKind::LevelOfEffort);
        net.link("A", "B", RelationKind::FS, 0);
        net.link("A", "L", RelationKind::SS, 0);
        net.link("L", "B", RelationKind::FF, 0);
        let s = net.run();
        let l = net.get(&s, "L");
        assert_eq!((net.day(l.early_start), net.day(l.early_finish)), (0, 7));
        assert_eq!(l.total_float, None);
        assert_eq!(net.day(net.get(&s, "B").early_start), 3);
        assert!(net.get(&s, "A").critical && net.get(&s, "B").critical);
    }

    #[test]
    fn cpm_12_wbs_rollup_and_summary_activity() {
        let mut net = n1();
        let root = net.wbs[0].id;
        let child = WbsId::new();
        net.wbs.push(WbsNode {
            id: child,
            parent: Some(root),
            code: "1.1".into(),
            name: "Civil".into(),
            obs: None,
            sort_key: 1,
        });
        for c in ["B", "D"] {
            net.act_mut(c).wbs = child;
        }
        net.add("SUM", 0, ActivityKind::WbsSummary);
        net.act_mut("SUM").wbs = child;
        let s = net.run();
        let (a, b) = s.wbs_rollup[&child];
        assert_eq!((net.day(a), net.day(b)), (3, 12));
        let (a, b) = s.wbs_rollup[&root];
        assert_eq!((net.day(a), net.day(b)), (0, 14));
        let sum = net.get(&s, "SUM");
        assert_eq!(
            (net.day(sum.early_start), net.day(sum.early_finish)),
            (3, 12)
        );
    }
}

mod validation {
    use super::*;

    #[test]
    fn cpm_02_cycle_is_reported_with_path() {
        let mut net = n1();
        net.link("F", "A", RelationKind::FS, 0);
        let err = schedule(&net.input()).unwrap_err();
        assert_eq!(err.code, ErrorCode::Cycle);
        let cycle: Vec<String> = serde_json::from_value(err.details["cycle"].clone()).unwrap();
        assert_eq!(cycle.first(), cycle.last(), "closed loop");
        assert!(cycle.contains(&"A".to_owned()) && cycle.contains(&"F".to_owned()));
    }

    #[test]
    fn cpm_01_rejects_bad_links() {
        let mut net = n1();
        net.link("A", "A", RelationKind::FS, 0);
        assert_eq!(
            schedule(&net.input()).unwrap_err().code,
            ErrorCode::Validation
        );

        let mut net = n1();
        net.link("A", "B", RelationKind::FS, 0);
        assert!(
            schedule(&net.input())
                .unwrap_err()
                .message
                .contains("duplicate")
        );

        let mut net = n1();
        net.rels
            .push(rel(ActivityId::new(), net.id("A"), RelationKind::FS, 0));
        assert_eq!(
            schedule(&net.input()).unwrap_err().code,
            ErrorCode::NotFound
        );
    }

    #[test]
    fn cpm_01_empty_project_schedules() {
        let net = Net::new(WorkWeek::continuous(), DAY);
        let s = net.run();
        assert_eq!(s.project_finish(), net.project.planned_start);
        assert_eq!(s.float_paths(3), Vec::<FloatPath>::new());
    }
}

// ───────────────────────────── property tests ─────────────────────────────

#[derive(Debug, Clone)]
struct RandNet {
    durs: Vec<i64>,
    links: Vec<(usize, usize, RelationKind, i64)>,
}

fn rand_net() -> impl Strategy<Value = RandNet> {
    (2usize..25).prop_flat_map(|n| {
        let durs = proptest::collection::vec(0i64..(10 * 24), n)
            .prop_map(|v| v.into_iter().map(|h| h * 60).collect::<Vec<_>>());
        let kinds = prop_oneof![
            Just(RelationKind::FS),
            Just(RelationKind::SS),
            Just(RelationKind::FF),
            Just(RelationKind::SF)
        ];
        let links = proptest::collection::vec((0..n, 0..n, kinds, -48i64..96), 0..(n * 3))
            .prop_map(|v| {
                let mut seen = std::collections::HashSet::new();
                v.into_iter()
                    .filter(|(a, b, k, _)| a < b && seen.insert((*a, *b, *k)))
                    .map(|(a, b, k, lag_h)| (a, b, k, lag_h * 60))
                    .collect::<Vec<_>>()
            });
        (durs, links).prop_map(|(durs, links)| RandNet { durs, links })
    })
}

impl RandNet {
    fn build(&self, week: WorkWeek, mpd: i64) -> Net {
        let mut net = Net::new(week, mpd);
        for (i, d) in self.durs.iter().enumerate() {
            net.add_minutes(&format!("N{i:02}"), *d, ActivityKind::TaskDependent);
        }
        for &(a, b, k, lag) in &self.links {
            net.link(&format!("N{a:02}"), &format!("N{b:02}"), k, lag);
        }
        net
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(300))]

    /// On a 24h calendar the passes reduce to plain longest-path arithmetic,
    /// so they must match a brute-force evaluation exactly.
    #[test]
    fn cpm_18_matches_brute_force_on_continuous_calendar(rn in rand_net()) {
        let net = rn.build(WorkWeek::continuous(), DAY);
        let s = net.run();
        let n = rn.durs.len();
        let start = net.project.planned_start.0;
        let mut es = vec![start; n];
        for j in 0..n {
            for &(a, b, k, lag) in &rn.links {
                if b != j { continue; }
                let ef_a = es[a] + rn.durs[a];
                let c = match k {
                    RelationKind::FS => ef_a + lag,
                    RelationKind::SS => es[a] + lag,
                    RelationKind::FF => ef_a + lag - rn.durs[j],
                    RelationKind::SF => es[a] + lag - rn.durs[j],
                };
                es[j] = es[j].max(c);
            }
        }
        let finish = (0..n).map(|i| es[i] + rn.durs[i]).max().unwrap();
        let mut lf = vec![finish; n];
        for i in (0..n).rev() {
            for &(a, b, k, lag) in &rn.links {
                if a != i { continue; }
                let ls_b = lf[b] - rn.durs[b];
                let c = match k {
                    RelationKind::FS => ls_b - lag,
                    RelationKind::SS => ls_b - lag + rn.durs[i],
                    RelationKind::FF => lf[b] - lag,
                    RelationKind::SF => lf[b] - lag + rn.durs[i],
                };
                lf[i] = lf[i].min(c);
            }
        }
        prop_assert_eq!(s.project_finish().0, finish);
        let mut min_tf = i64::MAX;
        for i in 0..n {
            let a = s.activity(i).unwrap();
            prop_assert_eq!(a.early_start.0, es[i], "ES N{:02}", i);
            prop_assert_eq!(a.late_finish.0, lf[i], "LF N{:02}", i);
            let tf = a.total_float.unwrap().0;
            prop_assert!(tf >= 0, "invariant 1: TF >= 0");
            prop_assert!(a.free_float.unwrap().0 <= tf);
            min_tf = min_tf.min(tf);
        }
        prop_assert_eq!(min_tf, 0, "invariant 1: some activity is critical");
    }

    /// On a working calendar every link holds and float stays non-negative.
    #[test]
    fn cpm_18_links_hold_on_working_calendar(rn in rand_net()) {
        let net = rn.build(WorkWeek::standard_5x8(), 480);
        let s = net.run();
        let cal = s.model.cal(0);
        for &(a, b, k, lag) in &rn.links {
            let (pa, pb) = (s.activity(a).unwrap(), s.activity(b).unwrap());
            let lag = WorkMinutes(lag);
            // Compared in working time: a negative lag lands on a period start
            // while a finish lands on the previous period end — the same
            // working instant, different clock times.
            let (req, actual) = match k {
                RelationKind::FS => (cal.add_work(pa.early_finish, lag).unwrap(), pb.early_start),
                RelationKind::SS => (cal.add_work(pa.early_start, lag).unwrap(), pb.early_start),
                RelationKind::FF => (cal.add_work(pa.early_finish, lag).unwrap(), pb.early_finish),
                RelationKind::SF => (cal.add_work(pa.early_start, lag).unwrap(), pb.early_finish),
            };
            let ok = cal.work_between(req, actual).unwrap().0 >= 0;
            prop_assert!(ok, "invariant 3: {:?} N{:02}->N{:02}", k, a, b);
        }
        for i in 0..rn.durs.len() {
            let a = s.activity(i).unwrap();
            prop_assert!(a.early_start >= net.project.planned_start);
            prop_assert!(a.total_float.unwrap().0 >= 0, "invariant 1 on 5x8");
        }
    }
}

/// CPM-18 performance target: 10k activities / 50k links. Run with
/// `cargo test --release -p prswt-cpm -- --ignored`.
#[test]
#[ignore = "benchmark; run in release"]
#[allow(clippy::disallowed_methods)]
fn cpm_18_benchmark_10k_50k() {
    let n = 10_000usize;
    let mut net = Net::new(WorkWeek::standard_5x8(), 480);
    for i in 0..n {
        net.add_minutes(
            &format!("A{i}"),
            i64::try_from((i * 37) % 4800).unwrap(),
            ActivityKind::TaskDependent,
        );
    }
    let kinds = [
        RelationKind::FS,
        RelationKind::SS,
        RelationKind::FF,
        RelationKind::SF,
    ];
    let mut count = 0;
    'outer: for i in 0..n {
        for step in 1..=5usize {
            let j = i + step * 7 % 97 + step;
            if j >= n {
                continue;
            }
            let k = kinds[(i + step) % 4];
            let lag = i64::try_from((i * step) % 960).unwrap() - 240;
            net.link(&format!("A{i}"), &format!("A{j}"), k, lag);
            count += 1;
            if count == 50_000 {
                break 'outer;
            }
        }
    }
    let input = net.input();
    let model = ScheduleModel::compile(&input, 190 * 366).unwrap();
    let dur = model.base_durations();
    let na = vec![true; model.nodes.len()];
    let ea = vec![true; model.edges.len()];
    let v = Variant {
        dur: &dur,
        node_active: &na,
        edge_active: &ea,
    };
    model.run(v).unwrap();
    let t = std::time::Instant::now();
    let reps = 20u32;
    for _ in 0..reps {
        model.run(v).unwrap();
    }
    let per = t.elapsed() / reps;
    assert!(
        per.as_millis() < 10,
        "run took {per:?} for {n} activities / {count} links"
    );
}

// ─────────────────────────────── helpers ───────────────────────────────

/// Oracle network N1 on a 24h calendar.
fn n1() -> Net {
    let mut net = Net::new(WorkWeek::continuous(), DAY);
    for (c, d) in [("A", 3), ("B", 4), ("C", 2), ("D", 5), ("E", 3), ("F", 2)] {
        net.add(c, d, ActivityKind::TaskDependent);
    }
    net.link("A", "B", RelationKind::FS, 0);
    net.link("A", "C", RelationKind::FS, 0);
    net.link("B", "D", RelationKind::FS, 0);
    net.link("C", "D", RelationKind::FS, 2 * DAY);
    net.link("C", "E", RelationKind::SS, DAY);
    net.link("D", "F", RelationKind::FS, 0);
    net.link("E", "F", RelationKind::FF, DAY);
    net
}

struct Net {
    project: Project,
    cals: BTreeMap<CalendarId, CalendarDef>,
    wbs: Vec<WbsNode>,
    acts: Vec<Activity>,
    rels: Vec<Relationship>,
    mpd: i64,
}

impl Net {
    fn new(week: WorkWeek, mpd: i64) -> Self {
        let cal = CalendarId::new();
        let start = Instant::from_ymd_hm(2026, 1, 5, 0, 0).unwrap();
        let mut cals = BTreeMap::new();
        cals.insert(
            cal,
            CalendarDef {
                id: cal,
                name: "default".into(),
                base: None,
                week: Some(week),
                exceptions: BTreeMap::new(),
                minutes_per_day: mpd,
            },
        );
        let root = WbsNode {
            id: WbsId::new(),
            parent: None,
            code: "1".into(),
            name: "Project".into(),
            obs: None,
            sort_key: 0,
        };
        Self {
            project: Project {
                id: ProjectId::new(),
                portfolio: None,
                name: "Test".into(),
                data_date: start,
                planned_start: start,
                must_finish_by: None,
                default_calendar: cal,
                settings: ScheduleSettings::default(),
            },
            cals,
            wbs: vec![root],
            acts: Vec::new(),
            rels: Vec::new(),
            mpd,
        }
    }

    fn add_calendar(&mut self, week: WorkWeek) -> CalendarId {
        let id = CalendarId::new();
        self.cals.insert(
            id,
            CalendarDef {
                id,
                name: "extra".into(),
                base: None,
                week: Some(week),
                exceptions: BTreeMap::new(),
                minutes_per_day: self.mpd,
            },
        );
        id
    }

    fn add(&mut self, code: &str, days: i64, kind: ActivityKind) {
        self.add_minutes(code, days * self.mpd, kind);
    }

    fn add_minutes(&mut self, code: &str, minutes: i64, kind: ActivityKind) {
        self.acts.push(Activity {
            id: ActivityId::new(),
            code: code.into(),
            name: code.into(),
            wbs: self.wbs[0].id,
            kind,
            calendar: self.project.default_calendar,
            duration: Estimate::fixed(WorkMinutes(minutes)),
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
        });
    }

    fn link(&mut self, a: &str, b: &str, kind: RelationKind, lag: i64) {
        self.rels.push(rel(self.id(a), self.id(b), kind, lag));
    }

    fn constrain(&mut self, code: &str, kind: ConstraintKind, day: i64) {
        let date = Some(self.at(day));
        self.act_mut(code).primary_constraint = Some(Constraint { kind, date });
    }

    fn constrain_no_date(&mut self, code: &str, kind: ConstraintKind) {
        self.act_mut(code).primary_constraint = Some(Constraint { kind, date: None });
    }

    fn actual(&mut self, code: &str, start: Option<i64>, finish: Option<i64>) {
        let (s, f) = (start.map(|d| self.at(d)), finish.map(|d| self.at(d)));
        let a = self.act_mut(code);
        a.actual_start = s;
        a.actual_finish = f;
    }

    fn act_mut(&mut self, code: &str) -> &mut Activity {
        self.acts.iter_mut().find(|a| a.code == code).unwrap()
    }

    fn id(&self, code: &str) -> ActivityId {
        self.acts.iter().find(|a| a.code == code).unwrap().id
    }

    fn at(&self, day: i64) -> Instant {
        self.project.planned_start.plus_minutes(day * DAY)
    }

    fn day(&self, t: Instant) -> i64 {
        (t.0 - self.project.planned_start.0) / DAY
    }

    fn input(&self) -> ScheduleInput<'_> {
        ScheduleInput {
            project: &self.project,
            calendars: &self.cals,
            wbs: &self.wbs,
            activities: &self.acts,
            relationships: &self.rels,
        }
    }

    fn run(&self) -> Schedule {
        schedule(&self.input()).unwrap()
    }

    fn get(&self, s: &Schedule, code: &str) -> ActivitySchedule {
        s.activity(s.index_of_code(code).unwrap()).unwrap()
    }
}

fn rel(pred: ActivityId, succ: ActivityId, kind: RelationKind, lag: i64) -> Relationship {
    Relationship {
        id: RelationshipId::new(),
        pred,
        succ,
        kind,
        lag: WorkMinutes(lag),
        lag_calendar: None,
    }
}
