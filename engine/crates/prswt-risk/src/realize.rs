//! RSK-13: deterministic Potential → Realized transition
//! (`vault/02-Architecture/03-Pipeline.md` §B).
//!
//! `plan_realization` turns a risk into concrete, reviewable graph edits; the
//! state layer records them as events. `schedule_delta` reports what the
//! edits did to the schedule.

use std::collections::{BTreeMap, HashMap, HashSet};

use prswt_core::ids::{ActivityId, ExpenseId, RelationshipId, ResourceId, RiskId};
use prswt_core::model::{
    Accrual, Activity, Assignment, Effective, Estimate, Expense, ImpactMode, ImpactProfile,
    MitigationKind, MitigationPlan, MitigationStatus, Quantity, Relationship, Resource, Risk,
    RiskBinding, RiskStatus, WbsNode,
};
use prswt_core::{EngineError, ErrorCode, Instant, Money, WorkMinutes};
use prswt_cpm::Schedule;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::Serialize;

/// Which value to use for uncertain impacts when a risk is realized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "values")]
pub enum RealizationChoice {
    /// Most likely values.
    MostLikely,
    /// Pessimistic values.
    Pessimistic,
    /// One value per impact, in that impact's unit (minutes, factor, money).
    Values(Vec<Decimal>),
}

/// What realization reads.
#[derive(Debug, Clone, Copy)]
pub struct RealizeInput<'a> {
    /// Current schedule (for float-based targeting).
    pub schedule: &'a Schedule,
    /// WBS (for branch bindings).
    pub wbs: &'a [WbsNode],
    /// Activities.
    pub activities: &'a [Activity],
    /// Relationships.
    pub relationships: &'a [Relationship],
    /// Assignments (for resource bindings).
    pub assignments: &'a [Assignment],
    /// Resources (for capacity changes).
    pub resources: &'a [Resource],
    /// Risks.
    pub risks: &'a [Risk],
    /// Mitigation plans (contingent branches).
    pub mitigations: &'a [MitigationPlan],
    /// Current planned cost per activity (for cost-factor impacts).
    pub activity_costs: &'a BTreeMap<ActivityId, Money>,
}

/// Concrete edits that realizing a risk implies.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct RealizationPlan {
    /// Risk being realized.
    pub risk: Option<RiskId>,
    /// `(activity, old remaining, new remaining)`.
    pub remaining_changes: Vec<(ActivityId, WorkMinutes, WorkMinutes)>,
    /// Cost impacts as expense lines.
    pub expenses: Vec<Expense>,
    /// Replacement capacity profiles per resource.
    pub max_units_changes: Vec<(ResourceId, Vec<Effective<Decimal>>)>,
    /// Contingent-branch activities to add.
    pub add_activities: Vec<Activity>,
    /// Links to add (branch links and anchors).
    pub add_relationships: Vec<Relationship>,
    /// Links replaced by the branch.
    pub remove_relationships: Vec<RelationshipId>,
    /// Secondary risks raised to Active with this probability.
    pub triggered: Vec<(RiskId, Decimal)>,
}

/// Schedule comparison before/after a change.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScheduleDelta {
    /// Finish before.
    pub finish_before: Instant,
    /// Finish after.
    pub finish_after: Instant,
    /// Working minutes on the project calendar (negative = earlier).
    pub finish_shift: i64,
    /// Codes that became critical.
    pub newly_critical: Vec<String>,
    /// Codes that stopped being critical.
    pub no_longer_critical: Vec<String>,
    /// (code, total float lost in working minutes), largest first.
    pub float_consumed: Vec<(String, i64)>,
    /// Most critical float path before.
    pub driving_path_before: Vec<String>,
    /// Most critical float path after.
    pub driving_path_after: Vec<String>,
}

/// Turns `risk_id` into a [`RealizationPlan`] at `at`; fails with `RISK_NOT_REALIZABLE` when not allowed.
pub fn plan_realization(
    input: &RealizeInput<'_>,
    risk_id: RiskId,
    at: Instant,
    choice: &RealizationChoice,
) -> Result<RealizationPlan, EngineError> {
    let risk = input
        .risks
        .iter()
        .find(|r| r.id == risk_id)
        .ok_or_else(|| EngineError::not_found(format!("risk {risk_id}")))?;
    if !matches!(risk.status, RiskStatus::Potential | RiskStatus::Active) {
        return Err(not_realizable(risk, "it is not potential or active"));
    }
    if let Some(p) = risk.proximity
        && (at < p.window_start || at > p.window_end)
    {
        return Err(not_realizable(
            risk,
            "the date is outside its proximity window",
        ));
    }
    let s = input.schedule;
    let targets = resolve_targets(input, &risk.bindings);
    let pick_once = |t: &[ActivityId]| -> Vec<ActivityId> {
        t.iter()
            .copied()
            .min_by_key(|id| {
                let i = s.index_of(*id);
                (
                    i.and_then(|i| s.dates.tf[i]).unwrap_or(i64::MAX),
                    i.map(|i| s.model.nodes[i].code.clone()).unwrap_or_default(),
                )
            })
            .into_iter()
            .collect()
    };
    let mut remaining: HashMap<ActivityId, i64> = HashMap::new();
    let current = |id: ActivityId| -> i64 { s.index_of(id).map_or(0, |i| s.model.nodes[i].dur) };
    let mut plan = RealizationPlan {
        risk: Some(risk.id),
        ..RealizationPlan::default()
    };
    for (k, imp) in risk.pre.impacts.iter().enumerate() {
        let override_value = match choice {
            RealizationChoice::Values(v) => v.get(k).copied(),
            _ => None,
        };
        let pess = *choice == RealizationChoice::Pessimistic;
        let sel = |mode: ImpactMode| match mode {
            ImpactMode::EachActivity => targets.clone(),
            ImpactMode::Once => pick_once(&targets),
        };
        match imp {
            ImpactProfile::ScheduleDelay { delay, mode } => {
                let d = override_value
                    .and_then(|v| v.round().to_i64())
                    .unwrap_or_else(|| estimate_value(delay, pess));
                for t in sel(*mode) {
                    let r = remaining.entry(t).or_insert_with(|| current(t));
                    *r = (*r + d).max(0);
                }
            }
            ImpactProfile::DurationFactor { factor, mode } => {
                let f = override_value.unwrap_or_else(|| quantity_value(factor, pess));
                for t in sel(*mode) {
                    let r = remaining.entry(t).or_insert_with(|| current(t));
                    *r = (Decimal::from(*r) * f.max(Decimal::ZERO))
                        .round()
                        .to_i64()
                        .unwrap_or(*r);
                }
            }
            ImpactProfile::CostAdd { amount, mode } => {
                let a = override_value.unwrap_or_else(|| quantity_value(amount, pess));
                for t in sel(*mode) {
                    plan.expenses.push(risk_expense(risk, t, Money(a)));
                }
            }
            ImpactProfile::CostFactor { factor, mode } => {
                let f = override_value.unwrap_or_else(|| quantity_value(factor, pess));
                for t in sel(*mode) {
                    let base = input.activity_costs.get(&t).copied().unwrap_or(Money::ZERO);
                    plan.expenses
                        .push(risk_expense(risk, t, base * (f - Decimal::ONE)));
                }
            }
            ImpactProfile::ResourceDegradation {
                resource,
                factor,
                window_start,
                window_end,
            } => {
                let f = override_value.unwrap_or_else(|| quantity_value(factor, pess));
                let ids: Vec<ResourceId> = match resource {
                    Some(r) => vec![*r],
                    None => risk
                        .bindings
                        .iter()
                        .filter_map(|b| match b {
                            RiskBinding::Resource(r) => Some(*r),
                            _ => None,
                        })
                        .collect(),
                };
                for rid in ids {
                    let Some(res) = input.resources.iter().find(|r| r.id == rid) else {
                        continue;
                    };
                    plan.max_units_changes
                        .push((rid, degrade(&res.max_units, *window_start, *window_end, f)));
                }
            }
        }
    }
    let mut changes: Vec<_> = remaining
        .into_iter()
        .map(|(id, new)| (id, WorkMinutes(current(id)), WorkMinutes(new)))
        .collect();
    changes.sort_by_key(|c| c.0);
    plan.remaining_changes = changes;

    for m in input.mitigations.iter().filter(|m| {
        m.risk == risk.id
            && m.kind == MitigationKind::Contingent
            && matches!(m.status, MitigationStatus::Accepted)
    }) {
        plan.add_activities
            .extend(m.subgraph.activities.iter().cloned());
        plan.add_relationships
            .extend(m.subgraph.links.iter().cloned());
        for an in &m.subgraph.anchors {
            plan.add_relationships.push(Relationship {
                id: RelationshipId::new(),
                pred: an.pred,
                succ: an.succ,
                kind: an.kind,
                lag: an.lag,
                lag_calendar: None,
            });
            plan.remove_relationships.extend(an.replaces);
        }
        // The branch takes over: the replaced work is abandoned (zero
        // remaining) so it no longer counts toward the finish.
        for &id in &m.subgraph.replaces {
            let open = s
                .index_of(id)
                .is_some_and(|i| s.model.nodes[i].status != prswt_cpm::Status::Completed);
            if !open {
                continue;
            }
            plan.remaining_changes.retain(|c| c.0 != id);
            plan.remaining_changes
                .push((id, WorkMinutes(current(id)), WorkMinutes(0)));
        }
    }
    plan.remaining_changes.sort_by_key(|c| c.0);
    for t in &risk.secondary_triggers {
        if let Some(target) = input.risks.iter().find(|r| r.id == t.target) {
            plan.triggered.push((
                target.id,
                target.pre.probability.max(t.conditional_probability),
            ));
        }
    }
    Ok(plan)
}

impl RealizationPlan {
    /// Applies the plan to owned entity lists (used by tests and the state layer).
    pub fn apply(
        &self,
        activities: &mut Vec<Activity>,
        relationships: &mut Vec<Relationship>,
        expenses: &mut Vec<Expense>,
        resources: &mut [Resource],
        risks: &mut [Risk],
    ) {
        for (id, _, new) in &self.remaining_changes {
            if let Some(a) = activities.iter_mut().find(|a| a.id == *id) {
                a.remaining = Some(*new);
            }
        }
        expenses.extend(self.expenses.iter().cloned());
        for (rid, units) in &self.max_units_changes {
            if let Some(r) = resources.iter_mut().find(|r| r.id == *rid) {
                r.max_units.clone_from(units);
            }
        }
        activities.extend(self.add_activities.iter().cloned());
        let removed: HashSet<_> = self.remove_relationships.iter().collect();
        relationships.retain(|r| !removed.contains(&r.id));
        relationships.extend(self.add_relationships.iter().cloned());
        for r in risks.iter_mut() {
            if Some(r.id) == self.risk {
                r.status = RiskStatus::Realized;
            }
            if let Some((_, p)) = self.triggered.iter().find(|(id, _)| *id == r.id) {
                r.pre.probability = *p;
                if r.status == RiskStatus::Potential {
                    r.status = RiskStatus::Active;
                }
            }
        }
    }
}

/// Before/after comparison keyed by activity id.
pub fn schedule_delta(before: &Schedule, after: &Schedule, top: usize) -> ScheduleDelta {
    let pc = &after.model.calendars[after.model.project_cal];
    let finish_shift = pc
        .work_between(before.project_finish(), after.project_finish())
        .map_or(0, |w| w.0);
    let mut newly = Vec::new();
    let mut no_longer = Vec::new();
    let mut consumed = Vec::new();
    for (i, n) in after.model.nodes.iter().enumerate() {
        let Some(j) = before.index_of(n.id) else {
            if after.critical[i] {
                newly.push(n.code.clone());
            }
            continue;
        };
        match (before.critical[j], after.critical[i]) {
            (false, true) => newly.push(n.code.clone()),
            (true, false) => no_longer.push(n.code.clone()),
            _ => {}
        }
        if let (Some(a), Some(b)) = (before.dates.tf[j], after.dates.tf[i])
            && b < a
        {
            consumed.push((n.code.clone(), a - b));
        }
    }
    consumed.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    consumed.truncate(top);
    let path = |s: &Schedule| -> Vec<String> {
        s.float_paths(1)
            .first()
            .map(|p| {
                p.activities
                    .iter()
                    .map(|&i| s.model.nodes[i].code.clone())
                    .collect()
            })
            .unwrap_or_default()
    };
    ScheduleDelta {
        finish_before: before.project_finish(),
        finish_after: after.project_finish(),
        finish_shift,
        newly_critical: newly,
        no_longer_critical: no_longer,
        float_consumed: consumed,
        driving_path_before: path(before),
        driving_path_after: path(after),
    }
}

fn not_realizable(r: &Risk, why: &str) -> EngineError {
    EngineError::new(
        ErrorCode::RiskNotRealizable,
        format!("risk {} cannot be realized: {why}", r.code),
    )
}

fn resolve_targets(input: &RealizeInput<'_>, bindings: &[RiskBinding]) -> Vec<ActivityId> {
    let parent: HashMap<_, _> = input.wbs.iter().map(|w| (w.id, w.parent)).collect();
    let mut out: Vec<ActivityId> = Vec::new();
    let push = |id: ActivityId, out: &mut Vec<ActivityId>| {
        let live = input
            .activities
            .iter()
            .any(|a| a.id == id && !a.is_completed());
        if live && !out.contains(&id) {
            out.push(id);
        }
    };
    for b in bindings {
        match b {
            RiskBinding::Activity(id) => push(*id, &mut out),
            RiskBinding::WbsBranch(root) => {
                for a in input.activities {
                    let mut cur = Some(a.wbs);
                    let mut guard = 0;
                    while let Some(w) = cur {
                        if w == *root {
                            push(a.id, &mut out);
                            break;
                        }
                        cur = parent.get(&w).copied().flatten();
                        guard += 1;
                        if guard > parent.len() {
                            break;
                        }
                    }
                }
            }
            RiskBinding::Resource(r) => {
                for a in input.assignments.iter().filter(|a| a.resource == *r) {
                    push(a.activity, &mut out);
                }
            }
            RiskBinding::Global => {
                let preds: HashSet<ActivityId> =
                    input.relationships.iter().map(|r| r.pred).collect();
                for a in input.activities.iter().filter(|a| !preds.contains(&a.id)) {
                    push(a.id, &mut out);
                }
            }
        }
    }
    out
}

fn estimate_value(e: &Estimate, pessimistic: bool) -> i64 {
    match (e, pessimistic) {
        (Estimate::Triangular { p, .. } | Estimate::Pert { p, .. }, true) => p.0,
        (Estimate::Uniform { max, .. }, true) => max.0,
        (Estimate::Discrete { outcomes }, true) => {
            outcomes.iter().map(|o| o.0.0).max().unwrap_or(0)
        }
        _ => e.planning_value().0,
    }
}

fn quantity_value(q: &Quantity, pessimistic: bool) -> Decimal {
    match q {
        Quantity::Fixed { value } => *value,
        Quantity::Triangular { m, p, .. } | Quantity::Pert { m, p, .. } => {
            if pessimistic {
                *p
            } else {
                *m
            }
        }
        Quantity::Uniform { min, max } => {
            if pessimistic {
                *max
            } else {
                (*min + *max) / Decimal::TWO
            }
        }
    }
}

fn risk_expense(r: &Risk, activity: ActivityId, amount: Money) -> Expense {
    Expense {
        id: ExpenseId::new(),
        activity,
        cost_account: None,
        name: format!("risk:{}", r.code),
        budget: amount,
        actual: Money::ZERO,
        accrual: Accrual::Uniform,
    }
}

/// Inserts a window where capacity is multiplied by `factor`, restoring the
/// prior value afterwards.
fn degrade(
    units: &[Effective<Decimal>],
    from: Instant,
    to: Instant,
    factor: Decimal,
) -> Vec<Effective<Decimal>> {
    let at = |t: Instant| {
        units
            .iter()
            .filter(|e| e.from <= t)
            .max_by_key(|e| e.from)
            .map_or(Decimal::ZERO, |e| e.value)
    };
    let mut out: Vec<Effective<Decimal>> = units
        .iter()
        .filter(|e| e.from < from || e.from >= to)
        .cloned()
        .collect();
    for e in units.iter().filter(|e| e.from >= from && e.from < to) {
        out.push(Effective {
            from: e.from,
            value: e.value * factor,
        });
    }
    out.push(Effective {
        from,
        value: at(from) * factor,
    });
    out.push(Effective {
        from: to,
        value: at(to),
    });
    out.sort_by_key(|e| e.from);
    out.dedup_by_key(|e| e.from);
    out
}
