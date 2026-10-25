//! Oracle: `vault/05-Math/RES-EVM-oracles.md` §EVM.

use std::collections::BTreeMap;
use std::str::FromStr;

use prswt_core::ids::{AssignmentId, CalendarId, ProjectId, RelationshipId, ResourceId, WbsId};
use prswt_core::model::{
    ActivityKind, CalendarDef, Effective, Estimate, PercentCompleteType, Project, RelationKind,
    Relationship, Resource, ResourceKind, ScheduleSettings, SpreadCurve, WbsNode, WorkWeek,
};
use prswt_core::{WorkMinutes, ids::ActivityId};
use prswt_cpm::{ScheduleInput, schedule};
use prswt_resource::{ResourceContext, ResourceInput};

use super::*;

#[test]
fn evm_01_capture_and_pv() {
    let w = World::new();
    let bl = w.baseline();
    assert_eq!(bl.bac, money("1600"));
    let mon = w.start();
    assert_eq!(bl.pv(mon), Money::ZERO);
    assert_eq!(bl.pv(day(mon, 1)), money("200"));
    assert_eq!(bl.pv(day(mon, 3)), money("600"));
    assert_eq!(
        bl.pv(mon.plus_minutes(4 * 60)),
        money("100"),
        "half of Monday's work by 12:00"
    );
    assert_eq!(bl.pv(day(mon, 30)), money("1600"));
    assert_eq!(bl.pv_curve().last().unwrap().1, money("1600"));
}

#[test]
fn evm_02_03_04_oracle_report() {
    let mut w = World::new();
    let bl = w.baseline();
    w.acts[0].actual_start = Some(w.start());
    w.acts[0].pct_complete = Decimal::new(50, 0);
    w.assignments[0].actual_cost = money("500");
    let as_of = day(w.start(), 3);
    let r = evaluate(&bl, as_of, &w.acts, &w.assignments, &[], EacMethod::Cpi);
    assert_eq!(
        (r.pv, r.ev, r.ac),
        (money("600"), money("400"), money("500"))
    );
    assert_eq!((r.cv, r.sv), (money("-100"), money("-200")));
    assert_eq!(r.cpi, Some(dec("0.8")));
    assert_eq!(r.spi.map(|s| s.round_dp(4)), Some(dec("0.6667")));
    assert_eq!(r.eac, Some(money("2000")));
    assert_eq!(r.etc, Some(money("1500")));
    assert_eq!(r.vac, Some(money("-400")));
    assert_eq!(r.tcpi.map(|t| t.round_dp(4)), Some(dec("1.0909")));
    assert_eq!(
        r.earned_schedule,
        Some(2 * 480),
        "PV reaches 400 after 2 working days"
    );
    assert_eq!(r.actual_time, 3 * 480);
    assert_eq!(r.spi_t.map(|s| s.round_dp(4)), Some(dec("0.6667")));

    let r = evaluate(
        &bl,
        as_of,
        &w.acts,
        &w.assignments,
        &[],
        EacMethod::RemainingAtBudget,
    );
    assert_eq!(r.eac, Some(money("1700")));
    let r = evaluate(&bl, as_of, &w.acts, &w.assignments, &[], EacMethod::CpiSpi);
    assert_eq!(r.eac.map(Money::to_cents_rounded), Some(money("2750.00")));
}

#[test]
fn evm_04_safe_division() {
    let w = World::new();
    let bl = w.baseline();
    let r = evaluate(&bl, w.start(), &w.acts, &w.assignments, &[], EacMethod::Cpi);
    assert_eq!((r.cpi, r.spi, r.eac, r.spi_t), (None, None, None, None));
    assert_eq!(r.earned_schedule, Some(0));
}

#[test]
fn evm_02_completed_counts_full_value() {
    let mut w = World::new();
    let bl = w.baseline();
    w.acts[0].actual_start = Some(w.start());
    w.acts[0].actual_finish = Some(day(w.start(), 4));
    let r = evaluate(
        &bl,
        day(w.start(), 4),
        &w.acts,
        &w.assignments,
        &[],
        EacMethod::Cpi,
    );
    assert_eq!(r.ev, money("800"));
}

// ─────────────────────────────── helpers ───────────────────────────────

fn dec(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn money(s: &str) -> Money {
    Money(dec(s))
}

fn day(t: Instant, n: i64) -> Instant {
    Instant::from_day_index(t.day_index() + n)
}

/// A (4d) → B (4d) FS on 5x8, each 32h of a 25/h crew: BAC 1600.
struct World {
    project: Project,
    cals: BTreeMap<CalendarId, CalendarDef>,
    wbs: Vec<WbsNode>,
    acts: Vec<Activity>,
    rels: Vec<Relationship>,
    resources: Vec<Resource>,
    assignments: Vec<Assignment>,
}

impl World {
    fn new() -> Self {
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
        let mk = |code: &str| Activity {
            id: ActivityId::new(),
            code: code.into(),
            name: code.into(),
            wbs: wbs[0].id,
            kind: ActivityKind::TaskDependent,
            calendar: cal,
            duration: Estimate::fixed(WorkMinutes(4 * 480)),
            remaining: None,
            primary_constraint: None,
            secondary_constraint: None,
            actual_start: None,
            actual_finish: None,
            pct_complete: Decimal::ZERO,
            pct_type: PercentCompleteType::Physical,
        };
        let acts = vec![mk("A"), mk("B")];
        let rels = vec![Relationship {
            id: RelationshipId::new(),
            pred: acts[0].id,
            succ: acts[1].id,
            kind: RelationKind::FS,
            lag: WorkMinutes(0),
            lag_calendar: None,
        }];
        let resources = vec![Resource {
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
                value: [money("25"); 5],
            }],
            unit_of_measure: None,
            obs: None,
        }];
        let assignments = acts
            .iter()
            .map(|a| Assignment {
                id: AssignmentId::new(),
                activity: a.id,
                resource: resources[0].id,
                budget_units: dec("32"),
                units_per_hour: Decimal::ONE,
                rate_type: 0,
                cost_account: None,
                curve: SpreadCurve::Uniform,
                actual_units: Decimal::ZERO,
                actual_cost: Money::ZERO,
                remaining_units: None,
            })
            .collect();
        Self {
            project: Project {
                id: ProjectId::new(),
                portfolio: None,
                name: "E".into(),
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
                    minutes_per_day: 480,
                },
            )]),
            wbs,
            acts,
            rels,
            resources,
            assignments,
        }
    }

    fn start(&self) -> Instant {
        self.project.planned_start
    }

    fn baseline(&self) -> Baseline {
        let s = schedule(&ScheduleInput {
            project: &self.project,
            calendars: &self.cals,
            wbs: &self.wbs,
            activities: &self.acts,
            relationships: &self.rels,
        })
        .unwrap();
        let ctx = ResourceContext::new(ResourceInput {
            schedule: &s,
            resources: &self.resources,
            assignments: &self.assignments,
            expenses: &[],
            calendars: &self.cals,
        })
        .unwrap();
        let cost = ctx.cost_rollup(&self.wbs).unwrap();
        Baseline::capture(BaselineId::new(), "BL1", &s, &cost)
    }
}
