//! RSK-02..12: Monte Carlo over the compiled CPM network.
//!
//! Pipeline: `vault/02-Architecture/03-Pipeline.md` §C. Every uncertain
//! quantity owns a *slot* keyed by a stable hash of its entity id; its
//! uniform draw for iteration `n` comes from ChaCha8(seed, key) on stream
//! `n`. Results are therefore independent of thread count, and two branches
//! that share an entity see the same draws (common random numbers).

use std::collections::{BTreeMap, HashMap, HashSet};

use prswt_core::ids::{ActivityId, RelationshipId, RiskId};
use prswt_core::model::{
    Activity, Assignment, CorrelationGroup, CriticalDefinition, ImpactMode, ImpactProfile,
    MitigationKind, MitigationPlan, MitigationStatus, Relationship, Risk, RiskBinding, RiskStatus,
    UncertainVarRef,
};
use prswt_core::{EngineError, ErrorCode, Instant};
use prswt_cpm::{ScheduleInput, ScheduleModel, Status, Variant};
use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::{Rng, SeedableRng};
use serde::Serialize;

use crate::dist::{Dist, DistBuilder, dec_f64, int_f64, norm_cdf, norm_inv};
use crate::linalg::{cholesky, nearest_correlation, spearman_to_pearson};

/// Calendar horizon for simulation (long tails).
const SIM_HORIZON_DAYS: i64 = 60 * 366;
/// Cap on stored per-variable samples used for sensitivity (memory guard).
const MAX_SENSITIVITY_CELLS: usize = 20_000_000;
/// Upper bound on iterations per run (hostile-input guard).
pub const MAX_ITERATIONS: u32 = 200_000;
/// Floor on remaining capacity share for resource degradation.
const MIN_CAPACITY_FACTOR: f64 = 0.05;

/// Per-activity cost split used to scale time-dependent cost with duration.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct ActivityCost {
    /// Resource cost that scales with the sampled duration.
    pub time_dependent: f64,
    /// Cost that does not depend on duration.
    pub fixed: f64,
}

/// Everything a simulation reads.
#[derive(Debug, Clone, Copy)]
pub struct SimInput<'a> {
    /// Network and calendars.
    pub schedule: ScheduleInput<'a>,
    /// Risks.
    pub risks: &'a [Risk],
    /// Mitigations (preventive switch states; contingent branches).
    pub mitigations: &'a [MitigationPlan],
    /// Correlation groups.
    pub correlations: &'a [CorrelationGroup],
    /// Assignments (resource bindings).
    pub assignments: &'a [Assignment],
    /// Planned cost split per activity.
    pub costs: &'a BTreeMap<ActivityId, ActivityCost>,
}

/// Which risk states to simulate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MitigationMode {
    /// Pre-mitigation risk states (preventive plans ignored).
    Pre,
    /// Post-mitigation states for risks with an accepted preventive plan.
    Post,
}

/// Simulation settings.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SimConfig {
    /// Maximum iterations.
    pub iterations: u32,
    /// Random seed (same seed ⇒ same result).
    pub seed: u64,
    /// Pre- or post-mitigation.
    pub mitigation: MitigationMode,
    /// Stop early when P50 and P80 move less than these between batches.
    pub convergence: Option<Convergence>,
    /// Iterations per batch (convergence checks).
    pub batch: u32,
    /// Optional targets to report probabilities for.
    pub targets: Option<Targets>,
}

/// Stop rule.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Convergence {
    /// Allowed P50/P80 finish movement between batches (minutes).
    pub finish_minutes: i64,
    /// Allowed relative P80 cost movement between batches.
    pub cost_relative: f64,
}

/// Targets for probability reporting.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Targets {
    /// Finish target.
    pub finish_by: Option<Instant>,
    /// Budget target.
    pub budget: Option<f64>,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            iterations: 2_000,
            seed: 0x5052_5357,
            mitigation: MitigationMode::Post,
            convergence: None,
            batch: 500,
            targets: None,
        }
    }
}

#[derive(Debug, Clone)]
struct Slot {
    key: u64,
}

#[derive(Debug, Clone)]
struct DurVar {
    node: usize,
    slot: usize,
    dist: Dist,
    /// Remaining/planned ratio for in-progress activities.
    scale: f64,
    activity: ActivityId,
}

#[derive(Debug, Clone)]
enum Effect {
    Delay,
    DurationFactor,
    CostAdd,
    CostFactor,
}

#[derive(Debug, Clone)]
struct SimImpact {
    effect: Effect,
    slot: usize,
    dist: Dist,
    targets: Vec<usize>,
}

#[derive(Debug, Clone)]
struct SimRisk {
    id: RiskId,
    occ_slot: usize,
    probability: f64,
    impacts: Vec<SimImpact>,
    /// (risk index, conditional probability) this risk may trigger.
    triggers: Vec<(usize, f64)>,
    /// Contingent branches: nodes/edges to activate, edges to deactivate.
    activate_nodes: Vec<usize>,
    activate_edges: Vec<usize>,
    deactivate_edges: Vec<usize>,
    /// Cost of the contingent plans that run when this risk occurs.
    contingent_cost: f64,
}

#[derive(Debug, Clone)]
struct Group {
    slots: Vec<usize>,
    chol: Vec<f64>,
}

/// Compiled simulation model.
#[derive(Debug, Clone)]
pub struct SimModel {
    /// The compiled network (with dormant contingent branches).
    pub cpm: ScheduleModel,
    base_dur: Vec<i64>,
    base_node_active: Vec<bool>,
    base_edge_active: Vec<bool>,
    slots: Vec<Slot>,
    vars: Vec<DurVar>,
    risks: Vec<SimRisk>,
    /// Risk indices in trigger-DAG topological order.
    risk_order: Vec<usize>,
    groups: Vec<Group>,
    cost: Vec<ActivityCost>,
    /// Cost of the preventive plans paid up front (post-mitigation only).
    preventive_cost: f64,
    crit_threshold: i64,
    /// Problems found while compiling (e.g. risks without targets).
    pub warnings: Vec<String>,
}

/// Summary statistics of a sample.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Quantiles {
    /// 10th percentile.
    pub p10: f64,
    /// 20th percentile.
    pub p20: f64,
    /// Median.
    pub p50: f64,
    /// 80th percentile.
    pub p80: f64,
    /// 90th percentile.
    pub p90: f64,
    /// Mean.
    pub mean: f64,
    /// Sample standard deviation.
    pub std_dev: f64,
    /// Minimum.
    pub min: f64,
    /// Maximum.
    pub max: f64,
}

/// How strongly one activity's duration drives the finish.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Sensitivity {
    /// Activity.
    pub activity: ActivityId,
    /// Spearman rank correlation of its duration with the finish.
    pub spearman: f64,
    /// Share of iterations in which it was critical.
    pub criticality: f64,
    /// Criticality × Spearman.
    pub cruciality: f64,
}

/// Effect of one risk across iterations.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RiskStat {
    /// Risk.
    pub risk: RiskId,
    /// Share of iterations in which it occurred.
    pub occurrence_rate: f64,
    /// E[finish | occurs] − E[finish | not occurs], working-free minutes.
    pub finish_delta_minutes: f64,
    /// E[cost | occurs] − E[cost | not].
    pub cost_delta: f64,
}

/// Simulation output.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SimResult {
    /// Iterations run.
    pub iterations: u32,
    /// Stopped by the convergence rule.
    pub converged: bool,
    /// Finish as minutes since the epoch (`Instant.0`), so quantiles are instants.
    pub finish: Quantiles,
    /// Total cost statistics.
    pub cost: Quantiles,
    /// Finish histogram `(from, to, count)` in instant minutes.
    pub histogram: Vec<(i64, i64, u32)>,
    /// Criticality index per activity, highest first.
    pub criticality: Vec<(ActivityId, f64)>,
    /// Duration sensitivities, strongest first.
    pub sensitivity: Vec<Sensitivity>,
    /// Risks by finish impact, largest first.
    pub risk_tornado: Vec<RiskStat>,
    /// P(finish ≤ target).
    pub prob_finish_by: Option<f64>,
    /// P(cost ≤ budget).
    pub prob_within_budget: Option<f64>,
    /// P(both targets met).
    pub prob_meet_targets: Option<f64>,
    /// `(iterations, P50, P80)` after each batch.
    pub convergence_trace: Vec<(u32, i64, i64)>,
    /// Finish per iteration (for paired comparisons).
    pub finish_samples: Vec<i64>,
    /// Cost per iteration.
    pub cost_samples: Vec<f64>,
}

/// Distribution of per-iteration differences between two runs.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PairedDelta {
    /// Iterations compared.
    pub iterations: u32,
    /// Finish difference (minutes).
    pub finish_minutes: Quantiles,
    /// Cost difference.
    pub cost: Quantiles,
}

pub(crate) struct Iter {
    pub(crate) finish: i64,
    pub(crate) cost: f64,
    pub(crate) critical: Vec<bool>,
    pub(crate) durations: Vec<f32>,
    pub(crate) occurred: Vec<bool>,
}

impl SimModel {
    /// Builds the simulation model: dormant branches, uncertain variables, risk targets, correlation factors.
    #[allow(clippy::too_many_lines)]
    pub fn compile(input: &SimInput<'_>, mode: MitigationMode) -> Result<Self, EngineError> {
        let base = input.schedule;
        let mut acts: Vec<Activity> = base.activities.to_vec();
        let mut rels: Vec<Relationship> = base.relationships.to_vec();
        let base_acts = acts.len();
        let base_rels = rels.len();

        // Contingent branches become dormant nodes/edges.
        let mut branch_of: HashMap<
            usize,
            (Vec<ActivityId>, Vec<RelationshipId>, Vec<RelationshipId>),
        > = HashMap::new();
        for (mi, mp) in input.mitigations.iter().enumerate() {
            let usable = matches!(
                mp.status,
                MitigationStatus::Accepted | MitigationStatus::Executed
            );
            if mp.kind != MitigationKind::Contingent || !usable {
                continue;
            }
            let mut nodes = Vec::new();
            let mut edges = Vec::new();
            let mut replaced = Vec::new();
            for a in &mp.subgraph.activities {
                nodes.push(a.id);
                acts.push(a.clone());
            }
            for l in &mp.subgraph.links {
                edges.push(l.id);
                rels.push(l.clone());
            }
            for an in &mp.subgraph.anchors {
                let id = RelationshipId::new();
                edges.push(id);
                rels.push(Relationship {
                    id,
                    pred: an.pred,
                    succ: an.succ,
                    kind: an.kind,
                    lag: an.lag,
                    lag_calendar: None,
                });
                replaced.extend(an.replaces);
            }
            branch_of.insert(mi, (nodes, edges, replaced));
        }

        let input2 = ScheduleInput {
            activities: &acts,
            relationships: &rels,
            ..base
        };
        let cpm = ScheduleModel::compile(&input2, SIM_HORIZON_DAYS)?;
        let base_dur = cpm.base_durations();
        let mut base_node_active = vec![true; cpm.nodes.len()];
        for a in &acts[base_acts..] {
            if let Some(&i) = cpm.index.get(&a.id) {
                base_node_active[i] = false;
            }
        }
        let edge_index: HashMap<RelationshipId, usize> = cpm
            .edges
            .iter()
            .enumerate()
            .filter_map(|(i, e)| e.id.map(|id| (id, i)))
            .collect();
        let mut base_edge_active = vec![true; cpm.edges.len()];
        for r in &rels[base_rels..] {
            if let Some(&e) = edge_index.get(&r.id) {
                base_edge_active[e] = false;
            }
        }

        // Base deterministic run for "Once" targeting and degradation windows.
        let det = cpm
            .run(Variant {
                dur: &base_dur,
                node_active: &base_node_active,
                edge_active: &base_edge_active,
            })
            .map_err(EngineError::from)?;

        let mut builder = DistBuilder::default();
        // Streams are keyed by business codes, not UUIDs, so the same project
        // reproduces after a reload or re-import (codes are unique and stable).
        let act_code: HashMap<ActivityId, &str> =
            acts.iter().map(|a| (a.id, a.code.as_str())).collect();
        let risk_code: HashMap<RiskId, &str> = input
            .risks
            .iter()
            .map(|r| (r.id, r.code.as_str()))
            .collect();
        let mut slots: Vec<Slot> = Vec::new();
        let mut slot_of: HashMap<UncertainVarRef, usize> = HashMap::new();
        let mut new_slot = |r: UncertainVarRef, slots: &mut Vec<Slot>| -> usize {
            *slot_of.entry(r.clone()).or_insert_with(|| {
                slots.push(Slot {
                    key: var_key(&r, &act_code, &risk_code),
                });
                slots.len() - 1
            })
        };

        let mut vars = Vec::new();
        for a in &acts {
            let Some(&node) = cpm.index.get(&a.id) else {
                continue;
            };
            let n = &cpm.nodes[node];
            if n.status == Status::Completed || a.duration.is_deterministic() || !n.is_driving() {
                continue;
            }
            let planned = a.duration.planning_value().0;
            // Remaining differs from the estimate (progress, a realized risk,
            // an accepted crash): scale the distribution to the remaining.
            let scale = if planned > 0 && n.dur != planned {
                int_f64(n.dur) / int_f64(planned)
            } else {
                1.0
            };
            let slot = new_slot(
                UncertainVarRef::ActivityDuration { activity: a.id },
                &mut slots,
            );
            vars.push(DurVar {
                node,
                slot,
                dist: builder.estimate(&a.duration),
                scale,
                activity: a.id,
            });
        }

        // Resolve risk targets.
        let wbs_parent: HashMap<_, _> = base.wbs.iter().map(|w| (w.id, w.parent)).collect();
        let in_branch = |node_wbs, root| {
            let mut cur = Some(node_wbs);
            let mut guard = 0;
            while let Some(w) = cur {
                if w == root {
                    return true;
                }
                cur = wbs_parent.get(&w).copied().flatten();
                guard += 1;
                if guard > wbs_parent.len() {
                    break;
                }
            }
            false
        };
        let users_of = |r| -> Vec<usize> {
            input
                .assignments
                .iter()
                .filter(|a| a.resource == r)
                .filter_map(|a| cpm.index.get(&a.activity).copied())
                .collect()
        };
        let open_ends: Vec<usize> = (0..base_acts.min(cpm.nodes.len()))
            .filter(|&i| {
                cpm.nodes[i].is_driving()
                    && cpm
                        .out_edges(i)
                        .iter()
                        .all(|&e| !cpm.nodes[cpm.edges[e].succ].is_driving())
            })
            .collect();
        let schedulable = |i: &usize| {
            let n = &cpm.nodes[*i];
            n.is_driving() && n.status != Status::Completed
        };
        let resolve = |b: &RiskBinding| -> Vec<usize> {
            let mut v: Vec<usize> = match b {
                RiskBinding::Activity(id) => cpm.index.get(id).copied().into_iter().collect(),
                RiskBinding::WbsBranch(w) => (0..cpm.nodes.len())
                    .filter(|&i| in_branch(cpm.nodes[i].wbs, *w))
                    .collect(),
                RiskBinding::Resource(r) => users_of(*r),
                RiskBinding::Global => open_ends.clone(),
            };
            v.retain(schedulable);
            v
        };
        let pick_once = |targets: Vec<usize>| -> Vec<usize> {
            targets
                .into_iter()
                .min_by(|&a, &b| {
                    det.tf[a]
                        .unwrap_or(i64::MAX)
                        .cmp(&det.tf[b].unwrap_or(i64::MAX))
                        .then(cpm.nodes[a].code.cmp(&cpm.nodes[b].code))
                })
                .into_iter()
                .collect()
        };

        let risk_index: HashMap<RiskId, usize> = input
            .risks
            .iter()
            .enumerate()
            .map(|(i, r)| (r.id, i))
            .collect();
        let has_preventive: HashSet<RiskId> = input
            .mitigations
            .iter()
            .filter(|m| {
                m.kind == MitigationKind::Preventive
                    && matches!(
                        m.status,
                        MitigationStatus::Accepted | MitigationStatus::Executed
                    )
            })
            .map(|m| m.risk)
            .collect();
        let mut warnings = Vec::new();
        let mut risks = Vec::with_capacity(input.risks.len());
        for r in input.risks {
            r.validate()?;
            let live = matches!(r.status, RiskStatus::Potential | RiskStatus::Active)
                && r.proximity.is_none_or(|p| p.window_end >= cpm.data_date);
            let state = match (mode, &r.post) {
                (MitigationMode::Post, Some(post)) if has_preventive.contains(&r.id) => post,
                _ => &r.pre,
            };
            let mut targets: Vec<usize> = Vec::new();
            for b in &r.bindings {
                for t in resolve(b) {
                    if !targets.contains(&t) {
                        targets.push(t);
                    }
                }
            }
            if targets.is_empty() && live {
                warnings.push(format!("risk {} has no schedulable targets", r.code));
            }
            let occ_slot = new_slot(UncertainVarRef::RiskOccurrence { risk: r.id }, &mut slots);
            let mut impacts = Vec::new();
            for (k, imp) in state.impacts.iter().enumerate() {
                let slot = new_slot(
                    UncertainVarRef::RiskImpact {
                        risk: r.id,
                        index: u32::try_from(k).unwrap_or(u32::MAX),
                    },
                    &mut slots,
                );
                let pick = |mode: ImpactMode, t: &Vec<usize>| match mode {
                    ImpactMode::EachActivity => t.clone(),
                    ImpactMode::Once => pick_once(t.clone()),
                };
                let (effect, dist, tg) = match imp {
                    ImpactProfile::ScheduleDelay { delay, mode } => (
                        Effect::Delay,
                        builder.estimate(delay),
                        pick(*mode, &targets),
                    ),
                    ImpactProfile::DurationFactor { factor, mode } => (
                        Effect::DurationFactor,
                        builder.quantity(factor),
                        pick(*mode, &targets),
                    ),
                    ImpactProfile::CostAdd { amount, mode } => (
                        Effect::CostAdd,
                        builder.quantity(amount),
                        pick(*mode, &targets),
                    ),
                    ImpactProfile::CostFactor { factor, mode } => (
                        Effect::CostFactor,
                        builder.quantity(factor),
                        pick(*mode, &targets),
                    ),
                    ImpactProfile::ResourceDegradation {
                        resource,
                        factor,
                        window_start,
                        window_end,
                    } => {
                        let users: Vec<usize> = match resource {
                            Some(res) => users_of(*res),
                            None => targets.clone(),
                        };
                        let hit = users
                            .into_iter()
                            .filter(|&i| det.es[i] <= *window_end && det.ef[i] >= *window_start)
                            .filter(|i| schedulable(i))
                            .collect();
                        // Capacity share c ⇒ duration × 1/c.
                        let inv = match builder.quantity(factor) {
                            Dist::Fixed(c) => Dist::Fixed(1.0 / c.max(MIN_CAPACITY_FACTOR)),
                            other => {
                                warnings.push(format!(
                                    "risk {}: uncertain degradation factor sampled as capacity share",
                                    r.code
                                ));
                                other
                            }
                        };
                        (Effect::DurationFactor, inv, hit)
                    }
                };
                impacts.push(SimImpact {
                    effect,
                    slot,
                    dist,
                    targets: tg,
                });
            }
            let triggers = r
                .secondary_triggers
                .iter()
                .filter_map(|t| {
                    risk_index
                        .get(&t.target)
                        .map(|&j| (j, dec_f64(t.conditional_probability)))
                })
                .collect();
            let mut sr = SimRisk {
                id: r.id,
                occ_slot,
                probability: if live {
                    dec_f64(state.probability)
                } else {
                    0.0
                },
                impacts,
                triggers,
                activate_nodes: Vec::new(),
                activate_edges: Vec::new(),
                deactivate_edges: Vec::new(),
                contingent_cost: 0.0,
            };
            for (mi, (nodes, edges, replaced)) in &branch_of {
                if input.mitigations[*mi].risk != r.id {
                    continue;
                }
                sr.activate_nodes
                    .extend(nodes.iter().filter_map(|id| cpm.index.get(id).copied()));
                sr.activate_edges
                    .extend(edges.iter().filter_map(|id| edge_index.get(id).copied()));
                sr.deactivate_edges
                    .extend(replaced.iter().filter_map(|id| edge_index.get(id).copied()));
                sr.contingent_cost += dec_f64(input.mitigations[*mi].cost.0);
            }
            risks.push(sr);
        }
        let risk_order = risk_topo(&risks, input.risks)?;

        let mut groups = Vec::new();
        for g in input.correlations {
            g.validate()?;
            let n = g.members.len();
            if n < 2 {
                continue;
            }
            let members: Vec<usize> = g
                .members
                .iter()
                .filter_map(|m| slot_of.get(m).copied())
                .collect();
            if members.len() != n {
                warnings.push(format!(
                    "correlation group {} references variables that are not uncertain; ignored",
                    g.id
                ));
                continue;
            }
            let pearson: Vec<f64> = g
                .matrix
                .iter()
                .map(|&v| spearman_to_pearson(dec_f64(v)))
                .collect();
            let (fixed, repaired) = nearest_correlation(&pearson, n);
            if repaired {
                warnings.push(format!(
                    "correlation group {} was not positive semi-definite; repaired",
                    g.id
                ));
            }
            let chol = cholesky(&fixed, n).ok_or_else(|| {
                EngineError::validation(format!("correlation group {} is singular", g.id))
            })?;
            groups.push(Group {
                slots: members,
                chol,
            });
        }

        let cost = cpm
            .nodes
            .iter()
            .map(|n| input.costs.get(&n.id).copied().unwrap_or_default())
            .collect();
        // A preventive plan is paid whether or not the risk occurs, so it
        // only counts when the run assumes the plans are carried out.
        let preventive_cost = match mode {
            MitigationMode::Post => input
                .mitigations
                .iter()
                .filter(|m| {
                    m.kind == MitigationKind::Preventive && has_preventive.contains(&m.risk)
                })
                .map(|m| dec_f64(m.cost.0))
                .sum(),
            MitigationMode::Pre => 0.0,
        };
        let crit_threshold = match cpm.settings.critical {
            CriticalDefinition::TotalFloatAtMost(x) => x.0,
            CriticalDefinition::LongestPath => 0,
        };
        Ok(Self {
            cpm,
            base_dur,
            base_node_active,
            base_edge_active,
            slots,
            vars,
            risks,
            risk_order,
            groups,
            cost,
            preventive_cost,
            crit_threshold,
            warnings,
        })
    }

    /// Number of independent random inputs.
    pub fn variable_count(&self) -> usize {
        self.slots.len()
    }

    #[allow(clippy::cast_possible_truncation)]
    pub(crate) fn iterate(
        &self,
        seed: u64,
        n: u64,
        keep_durations: bool,
    ) -> Result<Iter, EngineError> {
        let mut u: Vec<f64> = self.slots.iter().map(|s| uniform(seed, s.key, n)).collect();
        for g in &self.groups {
            let k = g.slots.len();
            let z: Vec<f64> = g.slots.iter().map(|&s| norm_inv(u[s])).collect();
            for i in 0..k {
                let x: f64 = (0..=i).map(|j| g.chol[i * k + j] * z[j]).sum();
                u[g.slots[i]] = norm_cdf(x);
            }
        }
        let mut dur = self.base_dur.clone();
        let mut durations = Vec::new();
        for v in &self.vars {
            let sampled = v.dist.sample(u[v.slot]) * v.scale;
            dur[v.node] = to_minutes(sampled);
            if keep_durations {
                durations.push(sampled as f32);
            }
        }
        let mut node_active = self.base_node_active.clone();
        let mut edge_active = self.base_edge_active.clone();
        let mut occurred = vec![false; self.risks.len()];
        let mut raised = vec![0.0f64; self.risks.len()];
        let mut cost_add = vec![0.0f64; dur.len()];
        let mut cost_mult = vec![1.0f64; dur.len()];
        let mut mitigation_cost = self.preventive_cost;
        for &ri in &self.risk_order {
            let r = &self.risks[ri];
            let p = r.probability.max(raised[ri]);
            if u[r.occ_slot] >= p {
                continue;
            }
            occurred[ri] = true;
            mitigation_cost += r.contingent_cost;
            for &(j, cp) in &r.triggers {
                raised[j] = raised[j].max(cp);
            }
            for imp in &r.impacts {
                let x = imp.dist.sample(u[imp.slot]);
                for &t in &imp.targets {
                    match imp.effect {
                        Effect::Delay => dur[t] = (dur[t] + to_minutes(x)).max(0),
                        Effect::DurationFactor => {
                            dur[t] = to_minutes(int_f64(dur[t]) * x.max(0.0));
                        }
                        Effect::CostAdd => cost_add[t] += x,
                        Effect::CostFactor => cost_mult[t] *= x.max(0.0),
                    }
                }
            }
            for &i in &r.activate_nodes {
                node_active[i] = true;
            }
            for &e in &r.activate_edges {
                edge_active[e] = true;
            }
            for &e in &r.deactivate_edges {
                edge_active[e] = false;
            }
        }
        let dates = self
            .cpm
            .run(Variant {
                dur: &dur,
                node_active: &node_active,
                edge_active: &edge_active,
            })
            .map_err(EngineError::from)?;
        let mut cost = mitigation_cost;
        for i in 0..dur.len() {
            if !node_active[i] {
                continue;
            }
            let c = self.cost[i];
            let base = self.base_dur[i];
            let td = if base > 0 {
                c.time_dependent * int_f64(dur[i]) / int_f64(base)
            } else {
                c.time_dependent
            };
            cost += (td + c.fixed) * cost_mult[i] + cost_add[i];
        }
        let critical = (0..dur.len())
            .map(|i| node_active[i] && dates.tf[i].is_some_and(|t| t <= self.crit_threshold))
            .collect();
        Ok(Iter {
            finish: dates.project_finish.0,
            cost,
            critical,
            durations,
            occurred,
        })
    }

    /// Runs the simulation. Deterministic for a given model, seed and config.
    pub fn run(&self, cfg: &SimConfig) -> Result<SimResult, EngineError> {
        let total = cfg.iterations.clamp(1, MAX_ITERATIONS);
        let batch = cfg.batch.clamp(50, total.max(50));
        let keep_cap = MAX_SENSITIVITY_CELLS / self.vars.len().max(1);
        let mut all: Vec<Iter> = Vec::with_capacity(usize::try_from(total).unwrap_or(0));
        let mut trace = Vec::new();
        let mut stable = 0;
        let mut converged = false;
        let mut prev: Option<(i64, i64, f64)> = None;
        while u32::try_from(all.len()).unwrap_or(u32::MAX) < total {
            let start = u64::try_from(all.len()).unwrap_or(0);
            let end = (start + u64::from(batch)).min(u64::from(total));
            let chunk = run_range(self, cfg.seed, start, end, keep_cap)?;
            all.extend(chunk);
            let mut f: Vec<i64> = all.iter().map(|x| x.finish).collect();
            f.sort_unstable();
            let p50 = pick(&f, 50);
            let p80 = pick(&f, 80);
            let mut c: Vec<f64> = all.iter().map(|x| x.cost).collect();
            c.sort_by(f64::total_cmp);
            let c80 = pick_f(&c, 80);
            trace.push((u32::try_from(all.len()).unwrap_or(u32::MAX), p50, p80));
            if let (Some(conv), Some((q50, q80, qc))) = (cfg.convergence, prev) {
                let ok = (p50 - q50).abs() <= conv.finish_minutes
                    && (p80 - q80).abs() <= conv.finish_minutes
                    && (c80 - qc).abs() <= conv.cost_relative * qc.abs().max(1.0);
                stable = if ok { stable + 1 } else { 0 };
                if stable >= 2 {
                    converged = true;
                    break;
                }
            }
            prev = Some((p50, p80, c80));
        }
        Ok(self.aggregate(&all, cfg, converged, trace))
    }

    #[allow(clippy::cast_precision_loss)]
    fn aggregate(
        &self,
        all: &[Iter],
        cfg: &SimConfig,
        converged: bool,
        trace: Vec<(u32, i64, i64)>,
    ) -> SimResult {
        let n = all.len().max(1) as f64;
        let finish_samples: Vec<i64> = all.iter().map(|x| x.finish).collect();
        let cost_samples: Vec<f64> = all.iter().map(|x| x.cost).collect();
        let finish = quantiles(
            &finish_samples
                .iter()
                .map(|&x| int_f64(x))
                .collect::<Vec<_>>(),
        );
        let cost = quantiles(&cost_samples);

        let nodes = self.cpm.nodes.len();
        let mut crit_count = vec![0u32; nodes];
        for it in all {
            for (i, &c) in it.critical.iter().enumerate() {
                if c {
                    crit_count[i] += 1;
                }
            }
        }
        let ci: Vec<f64> = crit_count.iter().map(|&c| f64::from(c) / n).collect();
        let mut criticality: Vec<(ActivityId, f64)> = (0..nodes)
            .filter(|&i| ci[i] > 0.0)
            .map(|i| (self.cpm.nodes[i].id, ci[i]))
            .collect();
        criticality.sort_by(|a, b| b.1.total_cmp(&a.1));

        let finish_ranks = ranks(
            &finish_samples
                .iter()
                .map(|&x| int_f64(x))
                .collect::<Vec<_>>(),
        );
        let kept = all
            .iter()
            .take_while(|x| x.durations.len() == self.vars.len())
            .count();
        let mut sensitivity: Vec<Sensitivity> = self
            .vars
            .iter()
            .enumerate()
            .map(|(k, v)| {
                let xs: Vec<f64> = all[..kept]
                    .iter()
                    .map(|it| f64::from(it.durations[k]))
                    .collect();
                let rho = pearson(&ranks(&xs), &finish_ranks[..kept]);
                Sensitivity {
                    activity: v.activity,
                    spearman: rho,
                    criticality: ci[v.node],
                    cruciality: ci[v.node] * rho,
                }
            })
            .collect();
        sensitivity.sort_by(|a, b| b.spearman.abs().total_cmp(&a.spearman.abs()));

        let risk_tornado = self
            .risks
            .iter()
            .enumerate()
            .map(|(ri, r)| {
                let (mut fy, mut fn_, mut cy, mut cn, mut ny) = (0.0, 0.0, 0.0, 0.0, 0.0);
                for it in all {
                    if it.occurred[ri] {
                        fy += int_f64(it.finish);
                        cy += it.cost;
                        ny += 1.0;
                    } else {
                        fn_ += int_f64(it.finish);
                        cn += it.cost;
                    }
                }
                let nn = n - ny;
                let (df, dc) = if ny > 0.0 && nn > 0.0 {
                    (fy / ny - fn_ / nn, cy / ny - cn / nn)
                } else {
                    (0.0, 0.0)
                };
                RiskStat {
                    risk: r.id,
                    occurrence_rate: ny / n,
                    finish_delta_minutes: df,
                    cost_delta: dc,
                }
            })
            .collect::<Vec<_>>();
        let mut risk_tornado = risk_tornado;
        risk_tornado.sort_by(|a, b| {
            b.finish_delta_minutes
                .abs()
                .total_cmp(&a.finish_delta_minutes.abs())
        });

        let share = |f: &dyn Fn(&Iter) -> bool| all.iter().filter(|x| f(x)).count() as f64 / n;
        let (pf, pb, pj) = match cfg.targets {
            Some(t) => {
                let fin = |x: &Iter| t.finish_by.is_none_or(|d| x.finish <= d.0);
                let bud = |x: &Iter| t.budget.is_none_or(|b| x.cost <= b);
                (
                    t.finish_by.map(|_| share(&fin)),
                    t.budget.map(|_| share(&bud)),
                    Some(share(&|x| fin(x) && bud(x))),
                )
            }
            None => (None, None, None),
        };
        SimResult {
            iterations: u32::try_from(all.len()).unwrap_or(u32::MAX),
            converged,
            histogram: histogram(&finish_samples),
            finish,
            cost,
            criticality,
            sensitivity,
            risk_tornado,
            prob_finish_by: pf,
            prob_within_budget: pb,
            prob_meet_targets: pj,
            convergence_trace: trace,
            finish_samples,
            cost_samples,
        }
    }
}

/// RSK-12: per-iteration differences `b − a` (same seed ⇒ common random numbers).
pub fn paired_delta(a: &SimResult, b: &SimResult) -> PairedDelta {
    let n = a.finish_samples.len().min(b.finish_samples.len());
    let df: Vec<f64> = (0..n)
        .map(|i| int_f64(b.finish_samples[i] - a.finish_samples[i]))
        .collect();
    let dc: Vec<f64> = (0..n)
        .map(|i| b.cost_samples[i] - a.cost_samples[i])
        .collect();
    PairedDelta {
        iterations: u32::try_from(n).unwrap_or(u32::MAX),
        finish_minutes: quantiles(&df),
        cost: quantiles(&dc),
    }
}

#[cfg(feature = "parallel")]
fn run_range(
    m: &SimModel,
    seed: u64,
    start: u64,
    end: u64,
    keep_cap: usize,
) -> Result<Vec<Iter>, EngineError> {
    use rayon::prelude::*;
    (start..end)
        .into_par_iter()
        .map(|i| m.iterate(seed, i, usize::try_from(i).unwrap_or(usize::MAX) < keep_cap))
        .collect()
}

#[cfg(not(feature = "parallel"))]
fn run_range(
    m: &SimModel,
    seed: u64,
    start: u64,
    end: u64,
    keep_cap: usize,
) -> Result<Vec<Iter>, EngineError> {
    (start..end)
        .map(|i| m.iterate(seed, i, usize::try_from(i).unwrap_or(usize::MAX) < keep_cap))
        .collect()
}

/// Topological order of the secondary-trigger graph; cycles are rejected.
fn risk_topo(risks: &[SimRisk], src: &[Risk]) -> Result<Vec<usize>, EngineError> {
    let n = risks.len();
    let mut indeg = vec![0usize; n];
    for r in risks {
        for &(j, _) in &r.triggers {
            indeg[j] += 1;
        }
    }
    let mut queue: Vec<usize> = (0..n).filter(|&i| indeg[i] == 0).rev().collect();
    let mut order = Vec::with_capacity(n);
    while let Some(i) = queue.pop() {
        order.push(i);
        for &(j, _) in &risks[i].triggers {
            indeg[j] -= 1;
            if indeg[j] == 0 {
                queue.push(j);
            }
        }
    }
    if order.len() != n {
        let codes: Vec<&str> = (0..n)
            .filter(|&i| indeg[i] > 0)
            .map(|i| src[i].code.as_str())
            .collect();
        return Err(EngineError::new(
            ErrorCode::Cycle,
            format!(
                "secondary risk triggers form a loop among {}",
                codes.join(", ")
            ),
        ));
    }
    Ok(order)
}

/// Stable 64-bit FNV-1a over the variable's business identity (activity or
/// risk code; the UUID only when no code is known).
fn var_key(
    r: &UncertainVarRef,
    act_code: &HashMap<ActivityId, &str>,
    risk_code: &HashMap<RiskId, &str>,
) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    let risk_name = |id: &RiskId| {
        risk_code
            .get(id)
            .map_or_else(|| id.to_string(), |c| (*c).to_owned())
    };
    match r {
        UncertainVarRef::ActivityDuration { activity } => {
            eat(&[1]);
            let code = act_code
                .get(activity)
                .map_or_else(|| activity.to_string(), |c| (*c).to_owned());
            eat(code.as_bytes());
        }
        UncertainVarRef::RiskOccurrence { risk } => {
            eat(&[2]);
            eat(risk_name(risk).as_bytes());
        }
        UncertainVarRef::RiskImpact { risk, index } => {
            eat(&[3]);
            eat(risk_name(risk).as_bytes());
            eat(&index.to_le_bytes());
        }
    }
    h
}

/// Uniform in (0, 1) for (seed, variable, iteration).
fn uniform(seed: u64, key: u64, iteration: u64) -> f64 {
    let mut s = [0u8; 32];
    s[..8].copy_from_slice(&seed.to_le_bytes());
    s[8..16].copy_from_slice(&key.to_le_bytes());
    s[16..24].copy_from_slice(b"PRSWTmc1");
    let mut rng = ChaCha8Rng::from_seed(s);
    rng.set_stream(iteration);
    let x = rng.next_u64() >> 11;
    (int_f64(i64::try_from(x).unwrap_or(0)) + 0.5) / 9_007_199_254_740_992.0
}

#[allow(clippy::cast_possible_truncation)]
fn to_minutes(x: f64) -> i64 {
    if x.is_finite() {
        x.round().clamp(-1e15, 1e15) as i64
    } else {
        0
    }
}

/// Nearest-rank percentile of sorted data.
fn pick(sorted: &[i64], pct: usize) -> i64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = (pct * sorted.len()).div_ceil(100).saturating_sub(1);
    sorted[idx.min(sorted.len() - 1)]
}

fn pick_f(sorted: &[f64], pct: usize) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = (pct * sorted.len()).div_ceil(100).saturating_sub(1);
    sorted[idx.min(sorted.len() - 1)]
}

/// Summary statistics of `xs` (nearest-rank percentiles).
#[allow(clippy::cast_precision_loss)]
pub fn quantiles(xs: &[f64]) -> Quantiles {
    let mut s = xs.to_vec();
    s.sort_by(f64::total_cmp);
    let n = s.len().max(1) as f64;
    let mean = s.iter().sum::<f64>() / n;
    let var = s.iter().map(|x| (x - mean) * (x - mean)).sum::<f64>() / (n - 1.0).max(1.0);
    Quantiles {
        p10: pick_f(&s, 10),
        p20: pick_f(&s, 20),
        p50: pick_f(&s, 50),
        p80: pick_f(&s, 80),
        p90: pick_f(&s, 90),
        mean,
        std_dev: var.sqrt(),
        min: s.first().copied().unwrap_or(0.0),
        max: s.last().copied().unwrap_or(0.0),
    }
}

/// Average ranks (ties share the mean rank), 1-based.
#[allow(clippy::float_cmp)]
#[allow(clippy::cast_precision_loss)]
fn ranks(xs: &[f64]) -> Vec<f64> {
    let mut idx: Vec<usize> = (0..xs.len()).collect();
    idx.sort_by(|&a, &b| xs[a].total_cmp(&xs[b]));
    let mut r = vec![0.0; xs.len()];
    let mut i = 0;
    while i < idx.len() {
        let mut j = i;
        while j + 1 < idx.len() && xs[idx[j + 1]] == xs[idx[i]] {
            j += 1;
        }
        let avg = (i + j) as f64 / 2.0 + 1.0;
        for &k in &idx[i..=j] {
            r[k] = avg;
        }
        i = j + 1;
    }
    r
}

#[allow(clippy::cast_precision_loss)]
fn pearson(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len().min(b.len());
    if n < 2 {
        return 0.0;
    }
    let nf = n as f64;
    let ma = a[..n].iter().sum::<f64>() / nf;
    let mb = b[..n].iter().sum::<f64>() / nf;
    let (mut sab, mut saa, mut sbb) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let (x, y) = (a[i] - ma, b[i] - mb);
        sab += x * y;
        saa += x * x;
        sbb += y * y;
    }
    if saa <= 0.0 || sbb <= 0.0 {
        0.0
    } else {
        sab / (saa * sbb).sqrt()
    }
}

/// Freedman–Diaconis bins over finish samples: (from, to, count).
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn histogram(samples: &[i64]) -> Vec<(i64, i64, u32)> {
    if samples.is_empty() {
        return Vec::new();
    }
    let mut s = samples.to_vec();
    s.sort_unstable();
    let (lo, hi) = (s[0], s[s.len() - 1]);
    if lo == hi {
        return vec![(lo, hi, u32::try_from(s.len()).unwrap_or(u32::MAX))];
    }
    let iqr = int_f64(pick(&s, 75) - pick(&s, 25)).max(1.0);
    let width = (2.0 * iqr / (s.len() as f64).cbrt()).max(1.0);
    let bins = (int_f64(hi - lo) / width).ceil().clamp(1.0, 200.0) as i64;
    let w = (hi - lo + bins) / bins;
    let mut out: Vec<(i64, i64, u32)> = (0..bins)
        .map(|k| (lo + k * w, lo + (k + 1) * w, 0))
        .collect();
    for x in s {
        let k = usize::try_from(((x - lo) / w).min(bins - 1)).unwrap_or(0);
        out[k].2 += 1;
    }
    out
}
