//! ST-02: typed commands, the only way to change a [`ProjectState`].

use prswt_core::ids::{
    ActivityId, AssignmentId, CalendarId, CorrelationGroupId, ExpenseId, MitigationId,
    RelationshipId, ResourceId, RiskId, WbsId,
};
use prswt_core::model::{
    Activity, Assignment, BaselineMeta, CalendarDef, CorrelationGroup, Expense, MitigationPlan,
    RelationKind, Relationship, Resource, Risk, RiskBinding, RiskStatus, ScheduleSettings,
    UncertainVarRef, WbsNode,
};
use prswt_core::{EngineError, Instant, WorkMinutes};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::state::{EntityKind, EntityRef, ProjectState};

/// Upper bound on commands in one atomic batch (hostile-input guard).
pub const MAX_BATCH: usize = 10_000;

/// A typed change to project state.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Command {
    /// Edit project header fields (absent fields keep their value).
    UpdateProject {
        /// New name.
        #[serde(default)]
        name: Option<String>,
        /// New data date.
        #[serde(default)]
        data_date: Option<Instant>,
        /// New planned start.
        #[serde(default)]
        planned_start: Option<Instant>,
        /// New target finish.
        #[serde(default)]
        must_finish_by: Option<Instant>,
        /// Remove the target finish.
        #[serde(default)]
        clear_must_finish_by: bool,
        /// New scheduling options.
        #[serde(default)]
        settings: Option<ScheduleSettings>,
    },
    /// Create or replace a calendar.
    UpsertCalendar {
        /// The calendar.
        calendar: CalendarDef,
    },
    /// Delete an unused calendar.
    DeleteCalendar {
        /// Calendar id.
        id: CalendarId,
    },
    /// Create or replace a WBS element.
    UpsertWbs {
        /// The element.
        node: WbsNode,
    },
    /// Delete an empty WBS element.
    DeleteWbs {
        /// Element id.
        id: WbsId,
    },
    /// Create or replace an activity.
    UpsertActivity {
        /// The activity.
        activity: Box<Activity>,
    },
    /// Delete an activity with its links, assignments and expenses.
    DeleteActivity {
        /// Activity id.
        id: ActivityId,
        /// Link each predecessor to each successor (FS) so logic stays continuous.
        #[serde(default)]
        reconnect: bool,
    },
    /// Create or replace a relationship.
    UpsertRelationship {
        /// The relationship.
        relationship: Relationship,
    },
    /// Delete a relationship.
    DeleteRelationship {
        /// Relationship id.
        id: RelationshipId,
    },
    /// Create or replace a resource.
    UpsertResource {
        /// The resource.
        resource: Resource,
    },
    /// Delete a resource and its assignments.
    DeleteResource {
        /// Resource id.
        id: ResourceId,
    },
    /// Create or replace an assignment.
    UpsertAssignment {
        /// The assignment.
        assignment: Assignment,
    },
    /// Delete an assignment.
    DeleteAssignment {
        /// Assignment id.
        id: AssignmentId,
    },
    /// Create or replace an expense.
    UpsertExpense {
        /// The expense.
        expense: Expense,
    },
    /// Delete an expense.
    DeleteExpense {
        /// Expense id.
        id: ExpenseId,
    },
    /// Create or replace a risk.
    UpsertRisk {
        /// The risk.
        risk: Box<Risk>,
    },
    /// Delete a risk, its mitigations and references to it.
    DeleteRisk {
        /// Risk id.
        id: RiskId,
    },
    /// Change a risk's status.
    SetRiskStatus {
        /// Risk id.
        id: RiskId,
        /// New status.
        status: RiskStatus,
    },
    /// Create or replace a mitigation plan.
    UpsertMitigation {
        /// The plan.
        mitigation: Box<MitigationPlan>,
    },
    /// Delete a mitigation plan.
    DeleteMitigation {
        /// Plan id.
        id: MitigationId,
    },
    /// Create or replace a correlation group.
    UpsertCorrelation {
        /// The group.
        group: CorrelationGroup,
    },
    /// Delete a correlation group.
    DeleteCorrelation {
        /// Group id.
        id: CorrelationGroupId,
    },
    /// Record progress (absent fields unchanged; an actual finish completes the activity).
    UpdateProgress {
        /// Activity id.
        activity: ActivityId,
        /// Actual start.
        #[serde(default)]
        actual_start: Option<Instant>,
        /// Actual finish.
        #[serde(default)]
        actual_finish: Option<Instant>,
        /// Percent complete.
        #[serde(default)]
        pct_complete: Option<Decimal>,
        /// Remaining duration.
        #[serde(default)]
        remaining: Option<WorkMinutes>,
    },
    /// Record baseline metadata.
    CaptureBaseline {
        /// The baseline.
        baseline: BaselineMeta,
    },
    /// SYNC-02: set one entity to a whole value, or remove it (`None`).
    /// Cloud sync sends local changes this way; the batch is re-validated
    /// (typed decode + integrity) by the server like any other.
    SetEntity {
        /// Entity address.
        entity: EntityRef,
        /// New value as the entity's JSON, `None` to remove it.
        #[serde(default)]
        value: Option<serde_json::Value>,
    },
}

impl Command {
    /// Applies to `s` in place and returns every entity it touched.
    /// Integrity of the whole state is checked by the caller after the batch.
    #[allow(clippy::too_many_lines)]
    pub fn apply(&self, s: &mut ProjectState) -> Result<Vec<EntityRef>, EngineError> {
        use EntityKind as K;
        Ok(match self {
            Self::UpdateProject {
                name,
                data_date,
                planned_start,
                must_finish_by,
                clear_must_finish_by,
                settings,
            } => {
                let p = &mut s.project;
                if let Some(n) = name {
                    p.name.clone_from(n);
                }
                if let Some(d) = data_date {
                    p.data_date = *d;
                }
                if let Some(d) = planned_start {
                    p.planned_start = *d;
                }
                if *clear_must_finish_by {
                    p.must_finish_by = None;
                } else if must_finish_by.is_some() {
                    p.must_finish_by = *must_finish_by;
                }
                if let Some(x) = settings {
                    p.settings = *x;
                }
                vec![EntityRef::project()]
            }
            Self::UpsertCalendar { calendar } => {
                s.calendars.insert(calendar.id, calendar.clone());
                vec![EntityRef::new(K::Calendar, calendar.id)]
            }
            Self::DeleteCalendar { id } => {
                let used = s.project.default_calendar == *id
                    || s.activities.values().any(|a| a.calendar == *id)
                    || s.resources.values().any(|x| x.calendar == *id)
                    || s.calendars.values().any(|c| c.base == Some(*id));
                if used {
                    return Err(EngineError::validation("calendar is in use"));
                }
                require(s.calendars.remove(id).is_some(), "calendar", id)?;
                vec![EntityRef::new(K::Calendar, id)]
            }
            Self::UpsertWbs { node } => {
                s.wbs.insert(node.id, node.clone());
                vec![EntityRef::new(K::Wbs, node.id)]
            }
            Self::DeleteWbs { id } => {
                if s.wbs.values().any(|w| w.parent == Some(*id))
                    || s.activities.values().any(|a| a.wbs == *id)
                {
                    return Err(EngineError::validation(
                        "WBS node still has children or activities",
                    ));
                }
                require(s.wbs.remove(id).is_some(), "WBS node", id)?;
                let mut touched = vec![EntityRef::new(K::Wbs, id)];
                touched.extend(strip_bindings(s, |b| *b == RiskBinding::WbsBranch(*id)));
                touched
            }
            Self::UpsertActivity { activity } => {
                s.activities.insert(activity.id, (**activity).clone());
                vec![EntityRef::new(K::Activity, activity.id)]
            }
            Self::DeleteActivity { id, reconnect } => delete_activity(s, *id, *reconnect)?,
            Self::UpsertRelationship { relationship } => {
                s.relationships
                    .insert(relationship.id, relationship.clone());
                vec![EntityRef::new(K::Relationship, relationship.id)]
            }
            Self::DeleteRelationship { id } => {
                require(s.relationships.remove(id).is_some(), "relationship", id)?;
                vec![EntityRef::new(K::Relationship, id)]
            }
            Self::UpsertResource { resource } => {
                s.resources.insert(resource.id, resource.clone());
                vec![EntityRef::new(K::Resource, resource.id)]
            }
            Self::DeleteResource { id } => {
                require(s.resources.remove(id).is_some(), "resource", id)?;
                let mut touched = vec![EntityRef::new(K::Resource, id)];
                let gone: Vec<AssignmentId> = s
                    .assignments
                    .values()
                    .filter(|a| a.resource == *id)
                    .map(|a| a.id)
                    .collect();
                for a in gone {
                    s.assignments.remove(&a);
                    touched.push(EntityRef::new(K::Assignment, a));
                }
                touched.extend(strip_bindings(s, |b| *b == RiskBinding::Resource(*id)));
                touched
            }
            Self::UpsertAssignment { assignment } => {
                s.assignments.insert(assignment.id, assignment.clone());
                vec![EntityRef::new(K::Assignment, assignment.id)]
            }
            Self::DeleteAssignment { id } => {
                require(s.assignments.remove(id).is_some(), "assignment", id)?;
                vec![EntityRef::new(K::Assignment, id)]
            }
            Self::UpsertExpense { expense } => {
                s.expenses.insert(expense.id, expense.clone());
                vec![EntityRef::new(K::Expense, expense.id)]
            }
            Self::DeleteExpense { id } => {
                require(s.expenses.remove(id).is_some(), "expense", id)?;
                vec![EntityRef::new(K::Expense, id)]
            }
            Self::UpsertRisk { risk } => {
                s.risks.insert(risk.id, (**risk).clone());
                vec![EntityRef::new(K::Risk, risk.id)]
            }
            Self::DeleteRisk { id } => {
                require(s.risks.remove(id).is_some(), "risk", id)?;
                let mut touched = vec![EntityRef::new(K::Risk, id)];
                let plans: Vec<MitigationId> = s
                    .mitigations
                    .values()
                    .filter(|m| m.risk == *id)
                    .map(|m| m.id)
                    .collect();
                for m in plans {
                    s.mitigations.remove(&m);
                    touched.push(EntityRef::new(K::Mitigation, m));
                }
                let referencing: Vec<RiskId> = s
                    .risks
                    .values()
                    .filter(|x| x.secondary_triggers.iter().any(|t| t.target == *id))
                    .map(|x| x.id)
                    .collect();
                for rid in referencing {
                    if let Some(x) = s.risks.get_mut(&rid) {
                        x.secondary_triggers.retain(|t| t.target != *id);
                        touched.push(EntityRef::new(K::Risk, rid));
                    }
                }
                touched.extend(strip_members(s, |m| match m {
                    UncertainVarRef::RiskOccurrence { risk }
                    | UncertainVarRef::RiskImpact { risk, .. } => risk == id,
                    UncertainVarRef::ActivityDuration { .. } => false,
                }));
                touched
            }
            Self::SetRiskStatus { id, status } => {
                let x = s
                    .risks
                    .get_mut(id)
                    .ok_or_else(|| EngineError::not_found(format!("risk {id}")))?;
                x.status = *status;
                vec![EntityRef::new(K::Risk, id)]
            }
            Self::UpsertMitigation { mitigation } => {
                s.mitigations.insert(mitigation.id, (**mitigation).clone());
                vec![EntityRef::new(K::Mitigation, mitigation.id)]
            }
            Self::DeleteMitigation { id } => {
                require(s.mitigations.remove(id).is_some(), "mitigation", id)?;
                vec![EntityRef::new(K::Mitigation, id)]
            }
            Self::UpsertCorrelation { group } => {
                s.correlations.insert(group.id, group.clone());
                vec![EntityRef::new(K::Correlation, group.id)]
            }
            Self::DeleteCorrelation { id } => {
                require(s.correlations.remove(id).is_some(), "correlation group", id)?;
                vec![EntityRef::new(K::Correlation, id)]
            }
            Self::UpdateProgress {
                activity,
                actual_start,
                actual_finish,
                pct_complete,
                remaining,
            } => {
                let a = s
                    .activities
                    .get_mut(activity)
                    .ok_or_else(|| EngineError::not_found(format!("activity {activity}")))?;
                if actual_start.is_some() {
                    a.actual_start = *actual_start;
                }
                if actual_finish.is_some() {
                    a.actual_finish = *actual_finish;
                    a.pct_complete = Decimal::ONE_HUNDRED;
                    a.remaining = Some(WorkMinutes::ZERO);
                }
                if let Some(p) = pct_complete {
                    a.pct_complete = *p;
                }
                if remaining.is_some() {
                    a.remaining = *remaining;
                }
                vec![EntityRef::new(K::Activity, activity)]
            }
            Self::CaptureBaseline { baseline } => {
                s.baselines.insert(baseline.id, baseline.clone());
                vec![EntityRef::new(K::Baseline, baseline.id)]
            }
            Self::SetEntity { entity, value } => {
                // The project keeps its id: files, links and the store are
                // keyed by it. (Other ids are checked by `check_integrity`.)
                if entity.kind == EntityKind::Project
                    && let Some(v) = value
                    && v.get("id").and_then(serde_json::Value::as_str)
                        != Some(s.project.id.to_string().as_str())
                {
                    return Err(EngineError::validation("the project id cannot be changed"));
                }
                s.set_json(entity, value.clone())?;
                vec![entity.clone()]
            }
        })
    }
}

fn require(ok: bool, what: &str, id: impl std::fmt::Display) -> Result<(), EngineError> {
    if ok {
        Ok(())
    } else {
        Err(EngineError::not_found(format!("{what} {id}")))
    }
}

/// Removes an activity and everything that depends on it. With `reconnect`,
/// each predecessor is linked FS to each successor (ids derived
/// deterministically, so replaying the command is reproducible).
fn delete_activity(
    s: &mut ProjectState,
    id: ActivityId,
    reconnect: bool,
) -> Result<Vec<EntityRef>, EngineError> {
    use EntityKind as K;
    require(s.activities.remove(&id).is_some(), "activity", id)?;
    let mut touched = vec![EntityRef::new(K::Activity, id)];
    let links: Vec<Relationship> = s
        .relationships
        .values()
        .filter(|l| l.pred == id || l.succ == id)
        .cloned()
        .collect();
    for l in &links {
        s.relationships.remove(&l.id);
        touched.push(EntityRef::new(K::Relationship, l.id));
    }
    if reconnect {
        let preds: Vec<ActivityId> = links
            .iter()
            .filter(|l| l.succ == id)
            .map(|l| l.pred)
            .collect();
        let succs: Vec<ActivityId> = links
            .iter()
            .filter(|l| l.pred == id)
            .map(|l| l.succ)
            .collect();
        for &p in &preds {
            for &q in &succs {
                let exists = s
                    .relationships
                    .values()
                    .any(|l| l.pred == p && l.succ == q && l.kind == RelationKind::FS);
                if exists {
                    continue;
                }
                let rid = RelationshipId(derived_uuid(&[p.0, q.0, id.0]));
                s.relationships.insert(
                    rid,
                    Relationship {
                        id: rid,
                        pred: p,
                        succ: q,
                        kind: RelationKind::FS,
                        lag: WorkMinutes::ZERO,
                        lag_calendar: None,
                    },
                );
                touched.push(EntityRef::new(K::Relationship, rid));
            }
        }
    }
    let assignments: Vec<AssignmentId> = s
        .assignments
        .values()
        .filter(|a| a.activity == id)
        .map(|a| a.id)
        .collect();
    for a in assignments {
        s.assignments.remove(&a);
        touched.push(EntityRef::new(K::Assignment, a));
    }
    let expenses: Vec<ExpenseId> = s
        .expenses
        .values()
        .filter(|x| x.activity == id)
        .map(|x| x.id)
        .collect();
    for x in expenses {
        s.expenses.remove(&x);
        touched.push(EntityRef::new(K::Expense, x));
    }
    touched.extend(strip_bindings(s, |b| *b == RiskBinding::Activity(id)));
    touched.extend(strip_members(s, |m| {
        *m == UncertainVarRef::ActivityDuration { activity: id }
    }));
    Ok(touched)
}

fn strip_bindings(s: &mut ProjectState, gone: impl Fn(&RiskBinding) -> bool) -> Vec<EntityRef> {
    let ids: Vec<RiskId> = s
        .risks
        .values()
        .filter(|x| x.bindings.iter().any(&gone))
        .map(|x| x.id)
        .collect();
    let mut touched = Vec::new();
    for id in ids {
        if let Some(x) = s.risks.get_mut(&id) {
            x.bindings.retain(|b| !gone(b));
            if x.bindings.is_empty() {
                x.bindings.push(RiskBinding::Global);
            }
            touched.push(EntityRef::new(EntityKind::Risk, id));
        }
    }
    touched
}

fn strip_members(s: &mut ProjectState, gone: impl Fn(&UncertainVarRef) -> bool) -> Vec<EntityRef> {
    let ids: Vec<CorrelationGroupId> = s
        .correlations
        .values()
        .filter(|g| g.members.iter().any(&gone))
        .map(|g| g.id)
        .collect();
    let mut touched = Vec::new();
    for id in ids {
        let Some(g) = s.correlations.get(&id).cloned() else {
            continue;
        };
        let n = g.members.len();
        let keep: Vec<usize> = (0..n).filter(|&i| !gone(&g.members[i])).collect();
        if keep.len() < 2 {
            s.correlations.remove(&id);
        } else {
            let matrix = keep
                .iter()
                .flat_map(|&i| keep.iter().map(move |&j| (i, j)))
                .filter_map(|(i, j)| g.matrix.get(i * n + j).copied())
                .collect();
            let members = keep.iter().map(|&i| g.members[i].clone()).collect();
            s.correlations.insert(
                id,
                CorrelationGroup {
                    id,
                    members,
                    matrix,
                },
            );
        }
        touched.push(EntityRef::new(EntityKind::Correlation, id));
    }
    touched
}

/// Deterministic UUID from parts (FNV-1a 128-bit fold), version nibble = 8 (custom).
fn derived_uuid(parts: &[Uuid]) -> Uuid {
    let mut h: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    for p in parts {
        for b in p.as_bytes() {
            h ^= u128::from(*b);
            h = h.wrapping_mul(0x0000_0000_0100_0000_0000_0000_0000_013b);
        }
    }
    uuid::Builder::from_custom_bytes(h.to_be_bytes()).into_uuid()
}
