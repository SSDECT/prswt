//! Oracle: `vault/05-Math/RES-EVM-oracles.md` §Resources.

use std::collections::BTreeMap;
use std::str::FromStr;

use proptest::prelude::*;
use prswt_core::ids::{
    ActivityId, AssignmentId, CalendarId, CostAccountId, ExpenseId, ProjectId, RelationshipId,
    ResourceId, WbsId,
};
use prswt_core::model::{
    Activity, ActivityKind, CalendarDef, Effective, Estimate, PercentCompleteType, Project,
    RelationKind, Relationship, ResourceKind, ScheduleSettings, WbsNode, WorkWeek,
};
use prswt_cpm::{ScheduleInput, schedule};

use super::*;

const D8: i64 = 480;

mod curves {
    use super::*;

    #[test]
    fn res_01_cumulative_curves() {
        let x = dec("0.25");
        assert_eq!(cum_fraction(SpreadCurve::Uniform, x), dec("0.25"));
        assert_eq!(cum_fraction(SpreadCurve::FrontLoaded, x), dec("0.4375"));
        assert_eq!(cum_fraction(SpreadCurve::BackLoaded, x), dec("0.0625"));
        assert_eq!(cum_fraction(SpreadCurve::Bell, x), dec("0.125"));
        assert_eq!(cum_fraction(SpreadCurve::Bell, dec("0.75")), dec("0.875"));
        for c in [
            SpreadCurve::Uniform,
            SpreadCurve::FrontLoaded,
            SpreadCurve::BackLoaded,
            SpreadCurve::Bell,
        ] {
            assert_eq!(cum_fraction(c, Decimal::ZERO), Decimal::ZERO);
            assert_eq!(cum_fraction(c, Decimal::ONE), Decimal::ONE);
            assert_eq!(cum_fraction(c, dec("7")), Decimal::ONE, "clamped");
        }
    }

    #[test]
    fn res_01_spread_oracle_three_days() {
        let w = World::new(&[("A", 3)], &[]);
        let s = w.schedule();
        let i = s.index_of_code("A").unwrap();
        let (es, ef) = (s.dates.es[i], s.dates.ef[i]);
        let flat = spread_by_day(s.model.cal(i), es, ef, dec("24"), SpreadCurve::Uniform).unwrap();
        assert_eq!(
            flat.iter().map(|x| x.1).collect::<Vec<_>>(),
            [dec("8"), dec("8"), dec("8")]
        );
        let front =
            spread_by_day(s.model.cal(i), es, ef, dec("24"), SpreadCurve::FrontLoaded).unwrap();
        let total: Decimal = front.iter().map(|x| x.1).sum();
        assert_eq!(total, dec("24"), "exact telescoping sum");
        assert!(front[0].1 > front[1].1 && front[1].1 > front[2].1);
        assert_eq!(front[1].1.round_dp(6), dec("8"), "24*(8/9-5/9)");
    }

    proptest! {
        #[test]
        fn res_01_spread_sums_exactly(days in 1i64..20, units in 1i64..100_000, c in 0u8..4) {
            let curve = [SpreadCurve::Uniform, SpreadCurve::FrontLoaded, SpreadCurve::BackLoaded, SpreadCurve::Bell][usize::from(c)];
            let w = World::new(&[("A", days)], &[]);
            let s = w.schedule();
            let total = Decimal::new(units, 2);
            let parts = spread_by_day(s.model.cal(0), s.dates.es[0], s.dates.ef[0], total, curve).unwrap();
            prop_assert_eq!(parts.iter().map(|x| x.1).sum::<Decimal>(), total);
            prop_assert_eq!(i64::try_from(parts.len()).unwrap(), days, "one bucket per working day");
            prop_assert!(parts.iter().all(|x| x.1 >= Decimal::ZERO));
        }
    }
}

mod usage {
    use super::*;

    #[test]
    fn res_02_capacity_by_calendar_and_effective_dates() {
        let mut w = World::new(&[("A", 1)], &[]);
        let next_monday = Instant::from_day_index(w.monday().day_index() + 7);
        w.resources[0].max_units.push(Effective {
            from: next_monday,
            value: dec("2"),
        });
        let s = w.schedule();
        let ctx = w.ctx(&s);
        let r = &w.resources[0];
        let mon = w.monday().day_index();
        assert_eq!(ctx.capacity(r, mon).unwrap(), dec("8"));
        assert_eq!(ctx.capacity(r, mon + 5).unwrap(), Decimal::ZERO, "Saturday");
        assert_eq!(
            ctx.capacity(r, mon + 7).unwrap(),
            dec("16"),
            "max units doubled next week"
        );
    }

    #[test]
    fn res_03_histogram_and_overallocation() {
        let mut w = World::new(&[("A", 2), ("B", 2)], &[]);
        w.assign("A", "16");
        w.assign("B", "16");
        let s = w.schedule();
        let ctx = w.ctx(&s);
        let rid = w.resources[0].id;
        let h = ctx.histogram(Period::Day).unwrap();
        let mon = w.monday().day_index();
        assert_eq!(h[&rid][&mon], dec("16"));
        let week = ctx.histogram(Period::Week).unwrap();
        assert_eq!(week[&rid][&Period::Week.key(mon)], dec("32"));
        assert_eq!(
            Period::Week.key(mon),
            Period::Week.key(mon + 6),
            "Mon..Sun same ISO week"
        );
        let over = ctx.overallocations().unwrap();
        assert_eq!(over.len(), 1);
        assert_eq!((over[0].from_day - mon, over[0].to_day - mon), (0, 1));
        assert_eq!(
            (over[0].peak_required, over[0].available),
            (dec("16"), dec("8"))
        );
    }

    #[test]
    fn res_05_resource_dependent_duration() {
        let mut w = World::new(&[("A", 1)], &[]);
        w.assign("A", "24");
        w.assignments[0].units_per_hour = dec("0.5");
        let refs: Vec<&Assignment> = w.assignments.iter().collect();
        assert_eq!(
            resource_dependent_duration(&refs),
            Some(WorkMinutes(48 * 60))
        );
        assert_eq!(resource_dependent_duration(&[]), None);
    }
}

mod leveling {
    use super::*;

    #[test]
    fn res_04_serial_sgs_delays_lower_priority() {
        let mut w = World::new(&[("A", 2), ("B", 2)], &[]);
        w.assign("A", "16");
        w.assign("B", "16");
        let s = w.schedule();
        let ctx = w.ctx(&s);
        let r = level(&ctx, &LevelOptions::default()).unwrap();
        let b = s.index_of_code("B").unwrap();
        assert_eq!(r.delayed, vec![(w.id("B"), WorkMinutes(2 * D8))]);
        assert_eq!(r.start[b].to_string(), "2026-10-28T08:00");
        assert_eq!(r.finish_date.to_string(), "2026-10-29T17:00");
        assert_eq!(r.unresolved, Vec::<ActivityId>::new());
    }

    #[test]
    fn res_04_leveling_respects_logic_of_delayed_predecessor() {
        let mut w = World::new(&[("A", 2), ("B", 2), ("C", 1)], &[("B", "C")]);
        w.assign("A", "16");
        w.assign("B", "16");
        let s = w.schedule();
        let ctx = w.ctx(&s);
        // A goes first by code, B is pushed to Wed–Thu, and unresourced C must
        // follow the *leveled* B, not its original dates.
        let opts = LevelOptions {
            priority: vec![PriorityKey::Code],
            ..LevelOptions::default()
        };
        let r = level(&ctx, &opts).unwrap();
        let (a, c) = (s.index_of_code("A").unwrap(), s.index_of_code("C").unwrap());
        assert_eq!(
            r.start[a].to_string(),
            "2026-10-26T08:00",
            "A first by code"
        );
        assert_eq!(
            r.start[c].to_string(),
            "2026-10-30T08:00",
            "C waits for delayed B"
        );
    }

    #[test]
    fn res_04_preserve_float_reports_unresolved() {
        let mut w = World::new(&[("A", 2), ("B", 2)], &[]);
        w.assign("A", "16");
        w.assign("B", "16");
        let s = w.schedule();
        let ctx = w.ctx(&s);
        let opts = LevelOptions {
            preserve_float: true,
            ..LevelOptions::default()
        };
        let r = level(&ctx, &opts).unwrap();
        assert_eq!(r.unresolved, vec![w.id("B")]);
        assert_eq!(r.delayed, Vec::new());
    }
}

mod cost {
    use super::*;

    #[test]
    fn cost_01_rates_expenses_and_rollups() {
        let mut w = World::new(&[("A", 2), ("B", 1)], &[("A", "B")]);
        w.assign("A", "16");
        let ca = CostAccountId::new();
        w.assignments[0].cost_account = Some(ca);
        w.expenses.push(Expense {
            id: ExpenseId::new(),
            activity: w.id("B"),
            cost_account: None,
            name: "Permit".into(),
            budget: Money(dec("100")),
            actual: Money::ZERO,
            accrual: Accrual::Start,
        });
        let s = w.schedule();
        let ctx = w.ctx(&s);
        let roll = ctx.cost_rollup(&w.wbs).unwrap();
        assert_eq!(roll.total, Money(dec("900")), "16h x 50 + 100");
        assert_eq!(roll.by_activity[&w.id("A")], Money(dec("800")));
        assert_eq!(roll.by_cost_account[&ca], Money(dec("800")));
        assert_eq!(roll.by_wbs[&w.wbs[0].id], Money(dec("900")));
        let mon = w.monday().day_index();
        assert_eq!(roll.by_day[&mon], Money(dec("400")));
        assert_eq!(
            roll.by_day[&(mon + 2)],
            Money(dec("100")),
            "expense on B's start"
        );
    }

    #[test]
    fn cost_01_rate_change_mid_activity() {
        let mut w = World::new(&[("A", 2)], &[]);
        w.assign("A", "16");
        let tue = w.monday().plus_minutes(1440);
        w.resources[0].rates.push(Effective {
            from: Instant::from_day_index(tue.day_index()),
            value: [Money(dec("80")); 5],
        });
        let s = w.schedule();
        let roll = w.ctx(&s).cost_rollup(&w.wbs).unwrap();
        assert_eq!(roll.total, Money(dec("1040")), "8h@50 + 8h@80");
    }
}

// ─────────────────────────────── helpers ───────────────────────────────

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

struct World {
    project: Project,
    cals: BTreeMap<CalendarId, CalendarDef>,
    wbs: Vec<WbsNode>,
    acts: Vec<Activity>,
    rels: Vec<Relationship>,
    resources: Vec<Resource>,
    assignments: Vec<Assignment>,
    expenses: Vec<Expense>,
}

impl World {
    /// 5x8 calendar, project starts Mon 2026-10-26 08:00; one labor resource
    /// with 1 unit/h and rate 50.
    fn new(acts: &[(&str, i64)], fs: &[(&str, &str)]) -> Self {
        let cal = CalendarId::new();
        let start = Instant::from_ymd_hm(2026, 10, 26, 8, 0).unwrap();
        let wbs = vec![WbsNode {
            id: WbsId::new(),
            parent: None,
            code: "1".into(),
            name: "P".into(),
            obs: None,
            sort_key: 0,
        }];
        let mut w = Self {
            project: Project {
                id: ProjectId::new(),
                portfolio: None,
                name: "R".into(),
                data_date: start,
                planned_start: start,
                must_finish_by: None,
                default_calendar: cal,
                settings: ScheduleSettings::default(),
            },
            cals: BTreeMap::from([(
                cal,
                CalendarDef {
                    id: cal,
                    name: "5x8".into(),
                    base: None,
                    week: Some(WorkWeek::standard_5x8()),
                    exceptions: BTreeMap::new(),
                    minutes_per_day: D8,
                },
            )]),
            acts: Vec::new(),
            rels: Vec::new(),
            resources: vec![Resource {
                id: ResourceId::new(),
                code: "CREW".into(),
                name: "Crew".into(),
                kind: ResourceKind::Labor,
                calendar: cal,
                max_units: vec![Effective {
                    from: Instant(0),
                    value: Decimal::ONE,
                }],
                rates: vec![Effective {
                    from: Instant(0),
                    value: [Money(dec("50")); 5],
                }],
                unit_of_measure: None,
                obs: None,
            }],
            assignments: Vec::new(),
            expenses: Vec::new(),
            wbs,
        };
        for (code, days) in acts {
            w.acts.push(Activity {
                id: ActivityId::new(),
                code: (*code).into(),
                name: (*code).into(),
                wbs: w.wbs[0].id,
                kind: ActivityKind::TaskDependent,
                calendar: cal,
                duration: Estimate::fixed(WorkMinutes(days * D8)),
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
        for (a, b) in fs {
            let (pred, succ) = (w.id(a), w.id(b));
            w.rels.push(Relationship {
                id: RelationshipId::new(),
                pred,
                succ,
                kind: RelationKind::FS,
                lag: WorkMinutes(0),
                lag_calendar: None,
            });
        }
        w
    }

    fn assign(&mut self, code: &str, units: &str) {
        self.assignments.push(Assignment {
            id: AssignmentId::new(),
            activity: self.id(code),
            resource: self.resources[0].id,
            budget_units: dec(units),
            units_per_hour: Decimal::ONE,
            rate_type: 0,
            cost_account: None,
            curve: SpreadCurve::Uniform,
            actual_units: Decimal::ZERO,
            actual_cost: Money::ZERO,
            remaining_units: None,
        });
    }

    fn id(&self, code: &str) -> ActivityId {
        self.acts.iter().find(|a| a.code == code).unwrap().id
    }

    fn monday(&self) -> Instant {
        self.project.planned_start
    }

    fn schedule(&self) -> Schedule {
        schedule(&ScheduleInput {
            project: &self.project,
            calendars: &self.cals,
            wbs: &self.wbs,
            activities: &self.acts,
            relationships: &self.rels,
        })
        .unwrap()
    }

    fn ctx<'a>(&'a self, s: &'a Schedule) -> ResourceContext<'a> {
        ResourceContext::new(ResourceInput {
            schedule: s,
            resources: &self.resources,
            assignments: &self.assignments,
            expenses: &self.expenses,
            calendars: &self.cals,
        })
        .unwrap()
    }
}
