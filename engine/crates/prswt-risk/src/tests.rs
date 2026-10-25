//! Oracle: `vault/05-Math/RISK-oracles.md`.
#![allow(clippy::cast_precision_loss, clippy::float_cmp)]

use std::collections::BTreeMap;
use std::str::FromStr;

use prswt_core::ids::{
    ActivityId, CalendarId, CorrelationGroupId, MitigationId, ProjectId, RelationshipId, RiskId,
    WbsId,
};
use prswt_core::model::{
    Activity, ActivityKind, Anchor, CalendarDef, CorrelationGroup, Estimate, ImpactMode,
    ImpactProfile, MitigationKind, MitigationPlan, MitigationStatus, PercentCompleteType, Project,
    RelationKind, Relationship, Risk, RiskBinding, RiskKind, RiskState, RiskStatus,
    ScheduleSettings, SecondaryTrigger, Subgraph, UncertainVarRef, WbsNode, WorkWeek,
};
use prswt_core::{ErrorCode, Instant, Money, WorkMinutes};
use prswt_cpm::ScheduleInput;
use rust_decimal::Decimal;

use crate::dist::{DistBuilder, beta_inv, ln_gamma, norm_cdf, norm_inv, reg_inc_beta};
use crate::linalg::{cholesky, jacobi_eigen, nearest_correlation, spearman_to_pearson};
use crate::sim::*;

const DAY: i64 = 1440;
const DAYF: f64 = 1440.0;

mod distributions {
    use super::*;

    #[test]
    fn rsk_01_triangular_inverse_cdf() {
        let d = DistBuilder::default().estimate(&tri(0, 5, 10));
        assert!((d.sample(0.5) - 5.0 * DAYF).abs() < 1e-9);
        assert!(d.sample(0.0).abs() < 1e-9);
        assert!((d.sample(1.0) - 10.0 * DAYF).abs() < 1e-9);
        // CDF(2) = (2-0)^2 / ((10-0)(5-0)) = 0.08
        assert!((d.sample(0.08) - 2.0 * DAYF).abs() < 1e-6);
        assert!((d.mean() - 5.0 * DAYF).abs() < 1e-9);
    }

    #[test]
    fn rsk_01_special_functions() {
        assert!((ln_gamma(5.0) - 24f64.ln()).abs() < 1e-12);
        assert!((ln_gamma(0.5) - std::f64::consts::PI.sqrt().ln()).abs() < 1e-12);
        assert!((reg_inc_beta(0.5, 2.0, 2.0) - 0.5).abs() < 1e-12);
        // I_x(1, b) = 1 - (1-x)^b
        assert!((reg_inc_beta(0.3, 1.0, 3.0) - (1.0 - 0.7f64.powi(3))).abs() < 1e-12);
        assert!((beta_inv(0.5, 2.0, 3.0) - 0.385_727_568).abs() < 1e-6);
        assert!((norm_inv(0.975) - 1.959_963_985).abs() < 1e-6);
        for p in [0.001, 0.02, 0.3, 0.5, 0.77, 0.999] {
            assert!((norm_cdf(norm_inv(p)) - p).abs() < 2e-7, "{p}");
        }
    }

    #[test]
    fn rsk_01_pert_moments() {
        let d = DistBuilder::default().estimate(&Estimate::Pert {
            o: WorkMinutes(0),
            m: WorkMinutes(2 * DAY),
            p: WorkMinutes(10 * DAY),
            lambda: 4,
        });
        // mean = (o + 4m + p) / 6 = 3 days
        assert!((d.mean() / DAYF - 3.0).abs() < 1e-3, "{}", d.mean() / DAYF);
        assert!(d.sample(0.0).abs() < 1e-9 && (d.sample(1.0) / DAYF - 10.0).abs() < 1e-9);
        let mut prev = -1.0;
        for k in 0..=100 {
            let x = d.sample(f64::from(k) / 100.0);
            assert!(x >= prev, "monotone");
            prev = x;
        }
    }
}

mod linear_algebra {
    use super::*;

    #[test]
    fn rsk_07_eigen_cholesky_and_repair() {
        let (mut vals, _) = jacobi_eigen(&[2.0, 1.0, 1.0, 2.0], 2);
        vals.sort_by(f64::total_cmp);
        assert!((vals[0] - 1.0).abs() < 1e-12 && (vals[1] - 3.0).abs() < 1e-12);
        let l = cholesky(&[4.0, 2.0, 2.0, 3.0], 2).unwrap();
        assert!((l[0] - 2.0).abs() < 1e-12 && (l[2] - 1.0).abs() < 1e-12);
        assert!((l[3] - 2f64.sqrt()).abs() < 1e-12);
        assert!(cholesky(&[1.0, 2.0, 2.0, 1.0], 2).is_none());

        let bad = [1.0, 0.9, -0.9, 0.9, 1.0, 0.9, -0.9, 0.9, 1.0];
        let (fixed, repaired) = nearest_correlation(&bad, 3);
        assert!(repaired);
        assert!(
            cholesky(&fixed, 3).is_some(),
            "repaired matrix is positive definite"
        );
        for i in 0..3 {
            assert!((fixed[i * 3 + i] - 1.0).abs() < 1e-9);
        }
        let (_, untouched) = nearest_correlation(&[1.0, 0.5, 0.5, 1.0], 2);
        assert!(!untouched);
        assert!((spearman_to_pearson(1.0) - 1.0).abs() < 1e-12);
        assert!(spearman_to_pearson(0.5) > 0.5);
    }
}

mod analytic {
    use super::*;

    #[test]
    fn rsk_14_serial_sum_of_triangulars() {
        let mut w = World::new();
        for c in ["A", "B", "C"] {
            w.add(c, tri(2, 4, 9));
        }
        w.fs("A", "B");
        w.fs("B", "C");
        let r = w.run(20_000);
        let mean = (r.finish.mean - w.start().0 as f64) / DAYF;
        assert!((mean - 15.0).abs() < 0.1, "mean {mean}");
        // var = 3·(a²+b²+c²−ab−ac−bc)/18 = 6.5
        let sd = r.finish.std_dev / DAYF;
        assert!((sd - 6.5f64.sqrt()).abs() < 0.08, "sd {sd}");
        assert!(r.criticality.iter().all(|(_, c)| (*c - 1.0).abs() < 1e-12));
    }

    #[test]
    fn rsk_14_parallel_max_of_uniforms() {
        let mut w = World::new();
        w.add("A", uni(0, 10));
        w.add("B", uni(0, 10));
        w.add_kind(
            "M",
            Estimate::fixed(WorkMinutes(0)),
            ActivityKind::FinishMilestone,
        );
        w.fs("A", "M");
        w.fs("B", "M");
        let r = w.run(20_000);
        let off = |x: f64| (x - w.start().0 as f64) / DAYF;
        assert!(
            (off(r.finish.mean) - 20.0 / 3.0).abs() < 0.08,
            "E[max] {}",
            off(r.finish.mean)
        );
        assert!(
            (off(r.finish.p50) - 10.0 * 0.5f64.sqrt()).abs() < 0.1,
            "P50 {}",
            off(r.finish.p50)
        );
        let ci: f64 = r
            .criticality
            .iter()
            .filter(|(id, _)| *id != w.id("M"))
            .map(|x| x.1)
            .sum();
        assert!(
            (ci - 1.0).abs() < 0.01,
            "exactly one branch critical per iteration"
        );
    }

    #[test]
    fn rsk_03_bernoulli_delay_risk() {
        let mut w = World::new();
        w.add("A", fixed(10));
        w.risk(
            "R1",
            "0.3",
            vec![RiskBinding::Activity(w.id("A"))],
            vec![delay(10)],
        );
        let r = w.run(20_000);
        let off = |x: f64| (x - w.start().0 as f64) / DAYF;
        assert!((r.risk_tornado[0].occurrence_rate - 0.3).abs() < 0.015);
        assert!((r.risk_tornado[0].finish_delta_minutes / DAYF - 10.0).abs() < 1e-9);
        assert_eq!((off(r.finish.p50), off(r.finish.p80)), (10.0, 20.0));
        assert!((off(r.finish.mean) - 13.0).abs() < 0.2);
    }

    #[test]
    fn rsk_07_copula_hits_target_rank_correlation() {
        let mut w = World::new();
        w.add("A", uni(0, 10));
        w.add("B", uni(0, 10));
        let (a, b) = (w.id("A"), w.id("B"));
        w.correlate(&[a, b], "0.8");
        let m = w.model(MitigationMode::Post);
        let n = 8_000u64;
        let (mut xs, mut ys) = (Vec::new(), Vec::new());
        for i in 0..n {
            let it = m.iterate(7, i, true).unwrap();
            xs.push(f64::from(it.durations[0]));
            ys.push(f64::from(it.durations[1]));
        }
        let rho = spearman(&xs, &ys);
        assert!((rho - 0.8).abs() < 0.03, "spearman {rho}");
        assert_eq!(m.warnings, Vec::<String>::new());
    }
}

mod invariants {
    use super::*;

    #[test]
    fn rsk_14_degenerate_inputs_equal_cpm() {
        let mut w = World::new();
        w.add("A", tri(4, 4, 4));
        w.add("B", fixed(3));
        w.fs("A", "B");
        w.risk(
            "R",
            "1",
            vec![RiskBinding::Activity(w.id("B"))],
            vec![delay(2)],
        );
        let r = w.run(500);
        let want = w.start().0 + 9 * DAY;
        assert!(r.finish_samples.iter().all(|&f| f == want));
        assert_eq!(r.finish.std_dev, 0.0);
    }

    #[test]
    fn rsk_08_thread_count_invariance() {
        let mut w = World::new();
        for c in ["A", "B", "C", "D"] {
            w.add(c, tri(1, 3, 8));
        }
        w.fs("A", "C");
        w.fs("B", "C");
        w.fs("C", "D");
        w.risk("R", "0.4", vec![RiskBinding::Global], vec![delay(3)]);
        let m = w.model(MitigationMode::Post);
        let cfg = SimConfig {
            iterations: 3_000,
            ..SimConfig::default()
        };
        let one = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        let four = rayon::ThreadPoolBuilder::new()
            .num_threads(4)
            .build()
            .unwrap();
        let a = one.install(|| m.run(&cfg)).unwrap();
        let b = four.install(|| m.run(&cfg)).unwrap();
        assert_eq!(a.finish_samples, b.finish_samples);
        assert_eq!(a.cost_samples, b.cost_samples);
        let c = m.run(&SimConfig { seed: 99, ..cfg }).unwrap();
        assert_ne!(a.finish_samples, c.finish_samples, "seed matters");
    }

    #[test]
    fn rsk_12_common_random_numbers() {
        let mut live = World::new();
        live.add("A", tri(2, 5, 12));
        live.add("B", tri(1, 2, 6));
        live.fs("A", "B");
        let mut sandbox = live.clone();
        let live_r = live.run(2_000);
        assert_eq!(
            paired_delta(&live_r, &live.run(2_000)).finish_minutes.max,
            0.0
        );
        // Sandbox adds a fixed 1d lag after A: every paired delta is exactly +1d.
        sandbox.rels[0].lag = WorkMinutes(DAY);
        let d = paired_delta(&live_r, &sandbox.run(2_000));
        assert_eq!((d.finish_minutes.min, d.finish_minutes.max), (DAYF, DAYF));
    }

    #[test]
    fn rsk_09_convergence_stops_early() {
        let mut w = World::new();
        w.add("A", fixed(5));
        let m = w.model(MitigationMode::Post);
        let cfg = SimConfig {
            iterations: 10_000,
            batch: 100,
            convergence: Some(Convergence {
                finish_minutes: 1,
                cost_relative: 0.001,
            }),
            ..SimConfig::default()
        };
        let r = m.run(&cfg).unwrap();
        assert!(r.converged);
        assert_eq!(
            r.iterations, 300,
            "batch 1 sets baseline, batches 2-3 stable"
        );
    }
}

mod risk_physics {
    use super::*;

    #[test]
    fn rsk_06_secondary_trigger() {
        let mut w = World::new();
        w.add("A", fixed(5));
        let b = vec![RiskBinding::Activity(w.id("A"))];
        let r1 = w.risk("R1", "1", b.clone(), vec![delay(1)]);
        let r2 = w.risk("R2", "0", b, vec![delay(2)]);
        w.risks[0].secondary_triggers.push(SecondaryTrigger {
            target: r2,
            conditional_probability: Decimal::ONE,
        });
        let r = w.run(200);
        let rate = |id| {
            r.risk_tornado
                .iter()
                .find(|s| s.risk == id)
                .unwrap()
                .occurrence_rate
        };
        assert_eq!((rate(r1), rate(r2)), (1.0, 1.0));
        assert_eq!(r.finish.p50 - w.start().0 as f64, 8.0 * DAYF);

        w.risks[1].secondary_triggers.push(SecondaryTrigger {
            target: r1,
            conditional_probability: Decimal::ONE,
        });
        let err = SimModel::compile(&w.input(), MitigationMode::Post).unwrap_err();
        assert_eq!(err.code, ErrorCode::Cycle);
    }

    #[test]
    fn rsk_05_contingent_branch_spliced_only_on_occurrence() {
        for (p, want) in [("1", 20), ("0", 10)] {
            let mut w = World::new();
            w.add("A", fixed(5));
            w.add("B", fixed(5));
            w.fs("A", "B");
            let rid = w.risk("R", p, vec![RiskBinding::Activity(w.id("A"))], vec![]);
            let x = w.make("X", fixed(10));
            let (a, b) = (w.id("A"), w.id("B"));
            w.contingent(
                rid,
                vec![x.clone()],
                vec![
                    Anchor {
                        pred: a,
                        succ: x.id,
                        kind: RelationKind::FS,
                        lag: WorkMinutes(0),
                        replaces: None,
                    },
                    Anchor {
                        pred: x.id,
                        succ: b,
                        kind: RelationKind::FS,
                        lag: WorkMinutes(0),
                        replaces: None,
                    },
                ],
            );
            let r = w.run(100);
            assert_eq!(
                (r.finish.p50 - w.start().0 as f64) / DAYF,
                f64::from(want),
                "p={p}"
            );
        }
    }

    #[test]
    fn rsk_05_contingent_branch_replaces_link() {
        let mut w = World::new();
        w.add("A", fixed(5));
        w.add("B", fixed(5));
        w.fs("A", "B");
        w.rels[0].lag = WorkMinutes(10 * DAY); // slow original path: 20d
        let replaced = w.rels[0].id;
        let rid = w.risk("R", "1", vec![RiskBinding::Activity(w.id("A"))], vec![]);
        let x = w.make("X", fixed(1));
        let (a, b) = (w.id("A"), w.id("B"));
        w.contingent(
            rid,
            vec![x.clone()],
            vec![
                Anchor {
                    pred: a,
                    succ: x.id,
                    kind: RelationKind::FS,
                    lag: WorkMinutes(0),
                    replaces: Some(replaced),
                },
                Anchor {
                    pred: x.id,
                    succ: b,
                    kind: RelationKind::FS,
                    lag: WorkMinutes(0),
                    replaces: None,
                },
            ],
        );
        let r = w.run(50);
        assert_eq!((r.finish.p50 - w.start().0 as f64) / DAYF, 11.0);
    }

    #[test]
    fn rsk_04_pre_vs_post_mitigation() {
        let mut w = World::new();
        w.add("A", fixed(10));
        let rid = w.risk(
            "R",
            "1",
            vec![RiskBinding::Activity(w.id("A"))],
            vec![delay(5)],
        );
        w.risks[0].post = Some(RiskState {
            probability: Decimal::ZERO,
            impacts: vec![delay(5)],
        });
        w.mitigations.push(MitigationPlan {
            id: MitigationId::new(),
            risk: rid,
            kind: MitigationKind::Preventive,
            status: MitigationStatus::Accepted,
            subgraph: Subgraph::default(),
            cost: Money::ZERO,
        });
        let fin = |mode| {
            let r = w
                .model(mode)
                .run(&SimConfig {
                    iterations: 100,
                    ..SimConfig::default()
                })
                .unwrap();
            (r.finish.p50 - w.start().0 as f64) / DAYF
        };
        assert_eq!(fin(MitigationMode::Pre), 15.0);
        assert_eq!(fin(MitigationMode::Post), 10.0);
    }

    #[test]
    fn rsk_04_mitigation_costs() {
        // Oracle (vault/05-Math/RISK-oracles.md, RSK-04 mitigation cost): planned
        // 1000; preventive plan 300 is paid only post-mitigation; the
        // contingent plan 50 is paid when its risk occurs (p = 1), in both
        // modes. Pre = 1050, Post = 1350, every iteration.
        let mut w = World::new();
        w.add("A", fixed(10));
        let a = w.id("A");
        w.costs.insert(
            a,
            ActivityCost {
                time_dependent: 0.0,
                fixed: 1000.0,
            },
        );
        let r1 = w.risk("R1", "0.5", vec![RiskBinding::Activity(a)], vec![]);
        w.risks[0].post = Some(RiskState {
            probability: Decimal::ZERO,
            impacts: vec![],
        });
        w.mitigations.push(MitigationPlan {
            id: MitigationId::new(),
            risk: r1,
            kind: MitigationKind::Preventive,
            status: MitigationStatus::Accepted,
            subgraph: Subgraph::default(),
            cost: Money(Decimal::from(300)),
        });
        let r2 = w.risk("R2", "1", vec![RiskBinding::Activity(a)], vec![]);
        w.contingent(r2, vec![], vec![]);
        w.mitigations[1].cost = Money(Decimal::from(50));
        let cost = |mode| {
            let r = w
                .model(mode)
                .run(&SimConfig {
                    iterations: 50,
                    ..SimConfig::default()
                })
                .unwrap();
            (r.cost.p10, r.cost.p80)
        };
        assert_eq!(cost(MitigationMode::Pre), (1050.0, 1050.0));
        assert_eq!(cost(MitigationMode::Post), (1350.0, 1350.0));
    }

    #[test]
    fn rsk_03_cost_scales_with_duration_and_targets() {
        let mut w = World::new();
        w.add("A", fixed(10));
        let a = w.id("A");
        w.costs.insert(
            a,
            ActivityCost {
                time_dependent: 1000.0,
                fixed: 100.0,
            },
        );
        w.risk("R", "1", vec![RiskBinding::Activity(a)], vec![delay(10)]);
        let m = w.model(MitigationMode::Post);
        let finish_by = w.start().plus_minutes(15 * DAY);
        let r = m
            .run(&SimConfig {
                iterations: 100,
                targets: Some(Targets {
                    finish_by: Some(finish_by),
                    budget: Some(5000.0),
                }),
                ..SimConfig::default()
            })
            .unwrap();
        assert!((r.cost.p50 - 2100.0).abs() < 1e-9);
        assert_eq!(
            (r.prob_finish_by, r.prob_within_budget, r.prob_meet_targets),
            (Some(0.0), Some(1.0), Some(0.0))
        );
    }

    #[test]
    fn rsk_02_wbs_and_global_bindings() {
        let mut w = World::new();
        w.add("A", fixed(5));
        w.add("B", fixed(5));
        w.add("C", fixed(1));
        w.fs("A", "C");
        w.fs("B", "C");
        // Global delay hits only the open end (C).
        w.risk("G", "1", vec![RiskBinding::Global], vec![delay(4)]);
        let r = w.run(10);
        assert_eq!((r.finish.p50 - w.start().0 as f64) / DAYF, 10.0);
        // WBS-branch "Once" delay picks the least-float activity (A, code order on tie).
        let mut w2 = w.clone();
        w2.risks.clear();
        let root = w2.wbs[0].id;
        w2.risk(
            "W",
            "1",
            vec![RiskBinding::WbsBranch(root)],
            vec![ImpactProfile::ScheduleDelay {
                delay: Estimate::fixed(WorkMinutes(2 * DAY)),
                mode: ImpactMode::Once,
            }],
        );
        let r = w2.run(10);
        assert_eq!((r.finish.p50 - w.start().0 as f64) / DAYF, 8.0);
    }
}

// ─────────────────────────────── helpers ───────────────────────────────

fn tri(o: i64, m: i64, p: i64) -> Estimate {
    Estimate::Triangular {
        o: WorkMinutes(o * DAY),
        m: WorkMinutes(m * DAY),
        p: WorkMinutes(p * DAY),
    }
}

fn uni(a: i64, b: i64) -> Estimate {
    Estimate::Uniform {
        min: WorkMinutes(a * DAY),
        max: WorkMinutes(b * DAY),
    }
}

fn fixed(d: i64) -> Estimate {
    Estimate::fixed(WorkMinutes(d * DAY))
}

fn delay(d: i64) -> ImpactProfile {
    ImpactProfile::ScheduleDelay {
        delay: fixed(d),
        mode: ImpactMode::EachActivity,
    }
}

#[allow(clippy::cast_precision_loss)]
fn spearman(a: &[f64], b: &[f64]) -> f64 {
    let rank = |v: &[f64]| {
        let mut idx: Vec<usize> = (0..v.len()).collect();
        idx.sort_by(|&x, &y| v[x].total_cmp(&v[y]));
        let mut r = vec![0.0; v.len()];
        for (k, &i) in idx.iter().enumerate() {
            r[i] = k as f64;
        }
        r
    };
    let (ra, rb) = (rank(a), rank(b));
    let n = ra.len() as f64;
    let m = (n - 1.0) / 2.0;
    let cov: f64 = ra.iter().zip(&rb).map(|(x, y)| (x - m) * (y - m)).sum();
    let var: f64 = ra.iter().map(|x| (x - m) * (x - m)).sum();
    cov / var
}

#[derive(Clone)]
struct World {
    project: Project,
    cals: BTreeMap<CalendarId, CalendarDef>,
    wbs: Vec<WbsNode>,
    acts: Vec<Activity>,
    rels: Vec<Relationship>,
    risks: Vec<Risk>,
    mitigations: Vec<MitigationPlan>,
    correlations: Vec<CorrelationGroup>,
    costs: BTreeMap<ActivityId, ActivityCost>,
}

impl World {
    fn new() -> Self {
        let cal = CalendarId::new();
        let start = Instant::from_ymd_hm(2026, 1, 5, 0, 0).unwrap();
        Self {
            project: Project {
                id: ProjectId::new(),
                portfolio: None,
                name: "MC".into(),
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
                    name: "24h".into(),
                    base: None,
                    week: Some(WorkWeek::continuous()),
                    exceptions: BTreeMap::new(),
                    minutes_per_day: DAY,
                },
            )]),
            wbs: vec![WbsNode {
                id: WbsId::new(),
                parent: None,
                code: "1".into(),
                name: "P".into(),
                obs: None,
                sort_key: 0,
            }],
            acts: Vec::new(),
            rels: Vec::new(),
            risks: Vec::new(),
            mitigations: Vec::new(),
            correlations: Vec::new(),
            costs: BTreeMap::new(),
        }
    }

    fn start(&self) -> Instant {
        self.project.planned_start
    }

    fn make(&self, code: &str, duration: Estimate) -> Activity {
        Activity {
            id: ActivityId::new(),
            code: code.into(),
            name: code.into(),
            wbs: self.wbs[0].id,
            kind: ActivityKind::TaskDependent,
            calendar: self.project.default_calendar,
            duration,
            remaining: None,
            primary_constraint: None,
            secondary_constraint: None,
            actual_start: None,
            actual_finish: None,
            pct_complete: Decimal::ZERO,
            pct_type: PercentCompleteType::Duration,
        }
    }

    fn add(&mut self, code: &str, duration: Estimate) {
        self.add_kind(code, duration, ActivityKind::TaskDependent);
    }

    fn add_kind(&mut self, code: &str, duration: Estimate, kind: ActivityKind) {
        let mut a = self.make(code, duration);
        a.kind = kind;
        self.acts.push(a);
    }

    fn id(&self, code: &str) -> ActivityId {
        self.acts.iter().find(|a| a.code == code).unwrap().id
    }

    fn fs(&mut self, a: &str, b: &str) {
        let (pred, succ) = (self.id(a), self.id(b));
        self.rels.push(Relationship {
            id: RelationshipId::new(),
            pred,
            succ,
            kind: RelationKind::FS,
            lag: WorkMinutes(0),
            lag_calendar: None,
        });
    }

    fn risk(
        &mut self,
        code: &str,
        p: &str,
        bindings: Vec<RiskBinding>,
        impacts: Vec<ImpactProfile>,
    ) -> RiskId {
        let id = RiskId::new();
        self.risks.push(Risk {
            id,
            code: code.into(),
            title: code.into(),
            owner: None,
            kind: RiskKind::Threat,
            status: RiskStatus::Potential,
            pre: RiskState {
                probability: Decimal::from_str(p).unwrap(),
                impacts,
            },
            post: None,
            bindings,
            proximity: None,
            correlation_group: None,
            secondary_triggers: Vec::new(),
        });
        id
    }

    fn contingent(&mut self, risk: RiskId, activities: Vec<Activity>, anchors: Vec<Anchor>) {
        self.mitigations.push(MitigationPlan {
            id: MitigationId::new(),
            risk,
            kind: MitigationKind::Contingent,
            status: MitigationStatus::Accepted,
            subgraph: Subgraph {
                activities,
                links: Vec::new(),
                anchors,
            },
            cost: Money::ZERO,
        });
    }

    fn correlate(&mut self, acts: &[ActivityId], rho: &str) {
        let r = Decimal::from_str(rho).unwrap();
        self.correlations.push(CorrelationGroup {
            id: CorrelationGroupId::new(),
            members: acts
                .iter()
                .map(|&activity| UncertainVarRef::ActivityDuration { activity })
                .collect(),
            matrix: vec![Decimal::ONE, r, r, Decimal::ONE],
        });
    }

    fn input(&self) -> SimInput<'_> {
        SimInput {
            schedule: ScheduleInput {
                project: &self.project,
                calendars: &self.cals,
                wbs: &self.wbs,
                activities: &self.acts,
                relationships: &self.rels,
            },
            risks: &self.risks,
            mitigations: &self.mitigations,
            correlations: &self.correlations,
            assignments: &[],
            costs: &self.costs,
        }
    }

    fn model(&self, mode: MitigationMode) -> SimModel {
        SimModel::compile(&self.input(), mode).unwrap()
    }

    fn run(&self, iterations: u32) -> SimResult {
        self.model(MitigationMode::Post)
            .run(&SimConfig {
                iterations,
                ..SimConfig::default()
            })
            .unwrap()
    }
}

mod realization {
    use super::*;
    use crate::realize::{
        RealizationChoice, RealizationPlan, RealizeInput, plan_realization, schedule_delta,
    };
    use prswt_core::EngineError;
    use prswt_core::model::{Proximity, Quantity};

    /// A(5) → B(5) chain beside D(9): D has 1d float.
    fn world() -> World {
        let mut w = World::new();
        w.add("A", fixed(5));
        w.add("B", fixed(5));
        w.add("D", fixed(9));
        w.fs("A", "B");
        w
    }

    fn sched(w: &World) -> prswt_cpm::Schedule {
        prswt_cpm::schedule(&w.input().schedule).unwrap()
    }

    fn plan(
        w: &World,
        s: &prswt_cpm::Schedule,
        risk: RiskId,
        choice: &RealizationChoice,
    ) -> Result<RealizationPlan, EngineError> {
        let costs = BTreeMap::from([(w.id("D"), Money(Decimal::from(1000)))]);
        plan_realization(
            &RealizeInput {
                schedule: s,
                wbs: &w.wbs,
                activities: &w.acts,
                relationships: &w.rels,
                assignments: &[],
                resources: &[],
                risks: &w.risks,
                mitigations: &w.mitigations,
                activity_costs: &costs,
            },
            risk,
            w.start(),
            choice,
        )
    }

    fn delay_risk(w: &mut World, code: &str) -> RiskId {
        let d = w.id("D");
        w.risk(
            code,
            "0.4",
            vec![RiskBinding::Activity(d)],
            vec![ImpactProfile::ScheduleDelay {
                delay: tri(1, 3, 6),
                mode: ImpactMode::EachActivity,
            }],
        )
    }

    #[test]
    fn rsk_13_realize_delay_shifts_critical_path() {
        let mut w = world();
        let d = w.id("D");
        let rid = delay_risk(&mut w, "R");
        let before = sched(&w);
        let p = plan(&w, &before, rid, &RealizationChoice::MostLikely).unwrap();
        assert_eq!(
            p.remaining_changes,
            vec![(d, WorkMinutes(9 * DAY), WorkMinutes(12 * DAY))]
        );
        let (mut acts, mut rels, mut exp) = (w.acts.clone(), w.rels.clone(), Vec::new());
        p.apply(&mut acts, &mut rels, &mut exp, &mut [], &mut w.risks);
        assert_eq!(w.risks[0].status, RiskStatus::Realized);
        w.acts = acts;
        let after = sched(&w);
        let delta = schedule_delta(&before, &after, 10);
        assert_eq!(delta.finish_shift, 2 * DAY);
        assert_eq!(delta.newly_critical, ["D"]);
        assert_eq!(delta.no_longer_critical, ["A", "B"]);
        assert_eq!(delta.float_consumed, vec![("D".to_owned(), DAY)]);
        assert_eq!(delta.driving_path_before, ["A", "B"]);
        assert_eq!(delta.driving_path_after, ["D"]);
    }

    #[test]
    fn rsk_13_realization_choices() {
        let mut w = world();
        let rid = delay_risk(&mut w, "R");
        let s = sched(&w);
        let p = plan(&w, &s, rid, &RealizationChoice::Pessimistic).unwrap();
        assert_eq!(p.remaining_changes[0].2, WorkMinutes(15 * DAY));
        let p = plan(
            &w,
            &s,
            rid,
            &RealizationChoice::Values(vec![Decimal::from(DAY)]),
        )
        .unwrap();
        assert_eq!(p.remaining_changes[0].2, WorkMinutes(10 * DAY));
    }

    #[test]
    fn rsk_13_guards_triggers_costs_and_branches() {
        let mut w = world();
        let (a, b, d) = (w.id("A"), w.id("B"), w.id("D"));
        let r2 = w.risk("R2", "0.1", vec![RiskBinding::Activity(a)], vec![]);
        let r1 = w.risk(
            "R1",
            "0.5",
            vec![RiskBinding::Activity(d)],
            vec![
                ImpactProfile::CostAdd {
                    amount: Quantity::Fixed {
                        value: Decimal::from(250),
                    },
                    mode: ImpactMode::EachActivity,
                },
                ImpactProfile::CostFactor {
                    factor: Quantity::Fixed {
                        value: Decimal::new(15, 1),
                    },
                    mode: ImpactMode::EachActivity,
                },
            ],
        );
        w.risks[1].secondary_triggers.push(SecondaryTrigger {
            target: r2,
            conditional_probability: Decimal::new(7, 1),
        });
        let x = w.make("X", fixed(2));
        let replaced = w.rels[0].id;
        w.contingent(
            r1,
            vec![x.clone()],
            vec![
                Anchor {
                    pred: a,
                    succ: x.id,
                    kind: RelationKind::FS,
                    lag: WorkMinutes(0),
                    replaces: Some(replaced),
                },
                Anchor {
                    pred: x.id,
                    succ: b,
                    kind: RelationKind::FS,
                    lag: WorkMinutes(0),
                    replaces: None,
                },
            ],
        );
        let s = sched(&w);
        let p = plan(&w, &s, r1, &RealizationChoice::MostLikely).unwrap();
        assert_eq!(p.expenses.len(), 2);
        assert_eq!(p.expenses[0].budget, Money(Decimal::from(250)));
        assert_eq!(
            p.expenses[1].budget,
            Money(Decimal::from(500)),
            "1000 x (1.5 - 1)"
        );
        assert_eq!(p.triggered, vec![(r2, Decimal::new(7, 1))]);
        assert_eq!(p.add_activities.len(), 1);
        assert_eq!(p.remove_relationships, vec![replaced]);
        let (mut acts, mut rels, mut exp) = (w.acts.clone(), w.rels.clone(), Vec::new());
        p.apply(&mut acts, &mut rels, &mut exp, &mut [], &mut w.risks);
        assert_eq!(w.risks[0].status, RiskStatus::Active);
        assert_eq!(w.risks[0].pre.probability, Decimal::new(7, 1));
        assert_eq!(rels.len(), 2);
        assert!(rels.iter().all(|r| r.id != replaced));

        let err = plan(&w, &s, r1, &RealizationChoice::MostLikely).unwrap_err();
        assert_eq!(err.code, ErrorCode::RiskNotRealizable, "already realized");
        w.risks[0].proximity = Some(Proximity {
            window_start: w.start().plus_minutes(DAY),
            window_end: w.start().plus_minutes(2 * DAY),
        });
        let err = plan(&w, &s, r2, &RealizationChoice::MostLikely).unwrap_err();
        assert_eq!(
            err.code,
            ErrorCode::RiskNotRealizable,
            "outside proximity window"
        );
    }
}

mod mitigation_proposals {
    use super::*;
    use crate::mitigate::{Edit, Strategy, propose};

    fn chain() -> (World, prswt_cpm::Schedule) {
        let mut w = World::new();
        w.add("A", fixed(5));
        w.add("B", fixed(5));
        w.fs("A", "B");
        let (a, b) = (w.id("A"), w.id("B"));
        w.risk(
            "R",
            "0.5",
            vec![RiskBinding::Activity(a), RiskBinding::Activity(b)],
            vec![delay(2)],
        );
        let s = prswt_cpm::schedule(&w.input().schedule).unwrap();
        (w, s)
    }

    fn money(n: i64) -> Money {
        Money(Decimal::from(n))
    }

    #[test]
    fn rsk_15_crash_and_fast_track() {
        let (w, s) = chain();
        let crash = Strategy::Crash {
            reduction_pct: Decimal::from(50),
            cost_per_day: money(100),
        };
        let p = propose(&s, &w.acts, &w.rels, &w.risks[0], &crash).unwrap();
        assert_eq!(p.kind, MitigationKind::Preventive);
        assert_eq!(p.edits.len(), 2);
        assert_eq!(
            p.edits[0],
            Edit::SetRemaining {
                activity: w.id("A"),
                from: WorkMinutes(5 * DAY),
                to: WorkMinutes(5 * DAY / 2),
            }
        );
        assert_eq!(p.cost, money(500), "2 x 2.5d x 100");

        let ft = Strategy::FastTrack {
            overlap_pct: Decimal::from(40),
        };
        let p = propose(&s, &w.acts, &w.rels, &w.risks[0], &ft).unwrap();
        assert_eq!(
            p.edits,
            vec![Edit::ChangeLink {
                id: w.rels[0].id,
                kind: RelationKind::SS,
                lag: WorkMinutes(3 * DAY),
            }]
        );
        let bad = Strategy::FastTrack {
            overlap_pct: Decimal::from(140),
        };
        assert!(propose(&s, &w.acts, &w.rels, &w.risks[0], &bad).is_err());
    }

    #[test]
    fn rsk_15_buffer_helper_and_alternative_path() {
        let (w, s) = chain();
        let (a, b) = (w.id("A"), w.id("B"));
        let buffer = Strategy::ContingencyBuffer {
            duration: WorkMinutes(DAY),
            before: None,
        };
        let p = propose(&s, &w.acts, &w.rels, &w.risks[0], &buffer).unwrap();
        assert!(p.edits.contains(&Edit::RemoveLink { id: w.rels[0].id }));
        assert_eq!(
            p.edits
                .iter()
                .filter(|e| matches!(e, Edit::AddLink { .. }))
                .count(),
            2
        );
        assert!(p.rationale.contains('B'));

        let helper = Strategy::AddResource {
            share_pct: Decimal::from(20),
            cost_per_day: money(10),
        };
        let p = propose(&s, &w.acts, &w.rels, &w.risks[0], &helper).unwrap();
        assert!(p.edits.contains(&Edit::SetRemaining {
            activity: a,
            from: WorkMinutes(5 * DAY),
            to: WorkMinutes(4 * DAY),
        }));
        assert_eq!(
            p.edits
                .iter()
                .filter(|e| matches!(e, Edit::AddActivity { .. }))
                .count(),
            2
        );

        let alt = Strategy::AlternativePath {
            duration: WorkMinutes(2 * DAY),
            cost: Money::ZERO,
        };
        let p = propose(&s, &w.acts, &w.rels, &w.risks[0], &alt).unwrap();
        assert_eq!(p.kind, MitigationKind::Contingent);
        assert_eq!(p.subgraph.activities.len(), 2);
        assert!(
            p.subgraph
                .anchors
                .iter()
                .any(|an| an.replaces == Some(w.rels[0].id) && an.succ == b)
        );
    }
}

/// RSK-14 performance target: 5k activities x 5k iterations < 10 s.
/// `cargo test --release -p prswt-risk -- --ignored`
#[test]
#[ignore = "benchmark; run in release"]
#[allow(clippy::disallowed_methods)]
fn rsk_14_benchmark_5k_by_5k() {
    let mut w = World::new();
    let n = 5_000usize;
    for i in 0..n {
        let m = i64::try_from(i % 7).unwrap() + 2;
        w.add(&format!("A{i}"), tri(m - 1, m, m + 4));
    }
    let kinds = [
        RelationKind::FS,
        RelationKind::SS,
        RelationKind::FF,
        RelationKind::FS,
    ];
    for i in 0..n {
        for step in 1..=5usize {
            let j = i + step * 3 + step % 2;
            if j < n {
                let (pred, succ) = (w.acts[i].id, w.acts[j].id);
                w.rels.push(Relationship {
                    id: RelationshipId::new(),
                    pred,
                    succ,
                    kind: kinds[step % 4],
                    lag: WorkMinutes(0),
                    lag_calendar: None,
                });
            }
        }
    }
    let ids: Vec<ActivityId> = w.acts.iter().step_by(50).map(|a| a.id).collect();
    for (k, id) in ids.iter().enumerate() {
        w.risk(
            &format!("R{k}"),
            "0.2",
            vec![RiskBinding::Activity(*id)],
            vec![delay(3)],
        );
    }
    let m = w.model(MitigationMode::Post);
    let t = std::time::Instant::now();
    let r = m
        .run(&SimConfig {
            iterations: 5_000,
            ..SimConfig::default()
        })
        .unwrap();
    let el = t.elapsed();
    assert_eq!(r.iterations, 5_000);
    assert!(el.as_secs_f64() < 10.0, "5k x 5k took {el:?}");
}

#[test]
fn rsk_03_remaining_override_scales_distribution() {
    let mut w = World::new();
    w.add("A", tri(10, 10, 10));
    w.acts[0].remaining = Some(WorkMinutes(25 * DAY));
    let r = w.run(50);
    assert_eq!((r.finish.p50 - w.start().0 as f64) / DAYF, 25.0);
}

#[test]
fn rsk_08_reproducible_across_reimport() {
    // Two independently built projects with the same codes (fresh UUIDs, as
    // after re-importing a file) must give identical samples for a seed.
    let build = || {
        let mut w = World::new();
        w.add("A", tri(2, 5, 12));
        w.add("B", tri(1, 2, 6));
        w.fs("A", "B");
        let b = w.id("B");
        w.risk("R1", "0.4", vec![RiskBinding::Activity(b)], vec![delay(3)]);
        w.run(500)
    };
    assert_eq!(build().finish_samples, build().finish_samples);
}
