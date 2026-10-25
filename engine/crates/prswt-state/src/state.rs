//! ST-01: the editable project state on persistent maps (O(1) clone).

use std::collections::{BTreeMap, HashMap, HashSet};

use imbl::OrdMap;
use prswt_core::ids::{
    ActivityId, AssignmentId, BaselineId, CalendarId, CorrelationGroupId, ExpenseId, MitigationId,
    RelationshipId, ResourceId, RiskId, WbsId,
};
use prswt_core::model::{
    Activity, Assignment, BaselineMeta, CalendarDef, CorrelationGroup, Expense, MitigationPlan,
    Project, Relationship, Resource, Risk, RiskBinding, UncertainVarRef, WbsNode,
};
use prswt_core::{EngineError, ErrorCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Collections an entity can live in. `Project` is the singleton header.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    /// Project header.
    Project,
    /// Calendars.
    Calendar,
    /// WBS elements.
    Wbs,
    /// Activities.
    Activity,
    /// Relationships.
    Relationship,
    /// Resources.
    Resource,
    /// Assignments.
    Assignment,
    /// Expenses.
    Expense,
    /// Risks.
    Risk,
    /// Mitigation plans.
    Mitigation,
    /// Correlation groups.
    Correlation,
    /// Baselines.
    Baseline,
}

impl EntityKind {
    /// Every kind, in a stable order.
    pub const ALL: [EntityKind; 12] = [
        Self::Project,
        Self::Calendar,
        Self::Wbs,
        Self::Activity,
        Self::Relationship,
        Self::Resource,
        Self::Assignment,
        Self::Expense,
        Self::Risk,
        Self::Mitigation,
        Self::Correlation,
        Self::Baseline,
    ];
}

/// Address of one entity.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct EntityRef {
    /// Collection.
    pub kind: EntityKind,
    /// Id as text (`project` for the header).
    pub id: String,
}

impl EntityRef {
    /// Reference to entity `id` of `kind`.
    pub fn new(kind: EntityKind, id: impl ToString) -> Self {
        Self {
            kind,
            id: id.to_string(),
        }
    }

    /// Reference to the project header.
    pub fn project() -> Self {
        Self::new(EntityKind::Project, "project")
    }
}

/// Full editable state of one project on one branch.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct ProjectState {
    /// Project header.
    pub project: Project,
    /// Calendars.
    pub calendars: OrdMap<CalendarId, CalendarDef>,
    /// WBS elements.
    pub wbs: OrdMap<WbsId, WbsNode>,
    /// Activities.
    pub activities: OrdMap<ActivityId, Activity>,
    /// Relationships.
    pub relationships: OrdMap<RelationshipId, Relationship>,
    /// Resources.
    pub resources: OrdMap<ResourceId, Resource>,
    /// Assignments.
    pub assignments: OrdMap<AssignmentId, Assignment>,
    /// Expenses.
    pub expenses: OrdMap<ExpenseId, Expense>,
    /// Risks.
    pub risks: OrdMap<RiskId, Risk>,
    /// Mitigation plans.
    pub mitigations: OrdMap<MitigationId, MitigationPlan>,
    /// Correlation groups.
    pub correlations: OrdMap<CorrelationGroupId, CorrelationGroup>,
    /// Baseline metadata.
    pub baselines: OrdMap<BaselineId, BaselineMeta>,
}

/// Plain owned vectors, the shape the math crates borrow.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// Calendars.
    pub calendars: BTreeMap<CalendarId, CalendarDef>,
    /// WBS elements.
    pub wbs: Vec<WbsNode>,
    /// Activities.
    pub activities: Vec<Activity>,
    /// Relationships.
    pub relationships: Vec<Relationship>,
    /// Resources.
    pub resources: Vec<Resource>,
    /// Assignments.
    pub assignments: Vec<Assignment>,
    /// Expenses.
    pub expenses: Vec<Expense>,
    /// Risks.
    pub risks: Vec<Risk>,
    /// Mitigation plans.
    pub mitigations: Vec<MitigationPlan>,
    /// Correlation groups.
    pub correlations: Vec<CorrelationGroup>,
}

macro_rules! for_each_collection {
    ($m:ident) => {
        $m!(Calendar, calendars, CalendarId);
        $m!(Wbs, wbs, WbsId);
        $m!(Activity, activities, ActivityId);
        $m!(Relationship, relationships, RelationshipId);
        $m!(Resource, resources, ResourceId);
        $m!(Assignment, assignments, AssignmentId);
        $m!(Expense, expenses, ExpenseId);
        $m!(Risk, risks, RiskId);
        $m!(Mitigation, mitigations, MitigationId);
        $m!(Correlation, correlations, CorrelationGroupId);
        $m!(Baseline, baselines, BaselineId);
    };
}

impl ProjectState {
    /// New state with one calendar and a root WBS element.
    pub fn new(project: Project, default_calendar: CalendarDef, root_wbs: WbsNode) -> Self {
        let mut s = Self {
            project,
            calendars: OrdMap::new(),
            wbs: OrdMap::new(),
            activities: OrdMap::new(),
            relationships: OrdMap::new(),
            resources: OrdMap::new(),
            assignments: OrdMap::new(),
            expenses: OrdMap::new(),
            risks: OrdMap::new(),
            mitigations: OrdMap::new(),
            correlations: OrdMap::new(),
            baselines: OrdMap::new(),
        };
        s.project.default_calendar = default_calendar.id;
        s.calendars.insert(default_calendar.id, default_calendar);
        s.wbs.insert(root_wbs.id, root_wbs);
        s
    }

    /// Copies into plain vectors for the math crates.
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            calendars: self
                .calendars
                .iter()
                .map(|(k, v)| (*k, v.clone()))
                .collect(),
            wbs: self.wbs.values().cloned().collect(),
            activities: self.activities.values().cloned().collect(),
            relationships: self.relationships.values().cloned().collect(),
            resources: self.resources.values().cloned().collect(),
            assignments: self.assignments.values().cloned().collect(),
            expenses: self.expenses.values().cloned().collect(),
            risks: self.risks.values().cloned().collect(),
            mitigations: self.mitigations.values().cloned().collect(),
            correlations: self.correlations.values().cloned().collect(),
        }
    }

    /// Entity as JSON (`None` = absent).
    pub fn get_json(&self, r: &EntityRef) -> Option<Value> {
        macro_rules! get {
            ($k:ident, $f:ident, $id:ident) => {
                if r.kind == EntityKind::$k {
                    let id: $id = r.id.parse().ok()?;
                    return self.$f.get(&id).and_then(|v| serde_json::to_value(v).ok());
                }
            };
        }
        if r.kind == EntityKind::Project {
            return serde_json::to_value(&self.project).ok();
        }
        for_each_collection!(get);
        None
    }

    /// Sets (or removes, with `None`) an entity from JSON. Used by undo,
    /// redo and merge, which work on recorded JSON values.
    pub fn set_json(&mut self, r: &EntityRef, value: Option<Value>) -> Result<(), EngineError> {
        fn parse<T: DeserializeOwned>(v: Value) -> Result<T, EngineError> {
            serde_json::from_value(v)
                .map_err(|e| EngineError::validation(format!("bad entity JSON: {e}")))
        }
        let bad_id = || EngineError::validation(format!("bad id {}", r.id));
        macro_rules! set {
            ($k:ident, $f:ident, $id:ident) => {
                if r.kind == EntityKind::$k {
                    let id: $id = r.id.parse().map_err(|_| bad_id())?;
                    match value {
                        Some(v) => {
                            self.$f.insert(id, parse(v)?);
                        }
                        None => {
                            self.$f.remove(&id);
                        }
                    }
                    return Ok(());
                }
            };
        }
        if r.kind == EntityKind::Project {
            let v = value.ok_or_else(|| EngineError::validation("project cannot be removed"))?;
            self.project = parse(v)?;
            return Ok(());
        }
        for_each_collection!(set);
        Err(EngineError::validation("unknown entity kind"))
    }

    /// Ids present in collection `kind`.
    pub fn keys(&self, kind: EntityKind) -> Vec<String> {
        macro_rules! keys {
            ($k:ident, $f:ident, $id:ident) => {
                if kind == EntityKind::$k {
                    return self.$f.keys().map(ToString::to_string).collect();
                }
            };
        }
        if kind == EntityKind::Project {
            return vec!["project".into()];
        }
        for_each_collection!(keys);
        Vec::new()
    }

    /// ST-02: referential integrity and structural rules over the whole state.
    pub fn check_integrity(&self) -> Result<(), EngineError> {
        // Every entity is filed under its own id (whole-entity writes could
        // otherwise store B under A's key).
        macro_rules! keyed {
            ($k:ident, $f:ident, $id:ident) => {
                if let Some((k, _)) = self.$f.iter().find(|(k, v)| **k != v.id) {
                    return Err(EngineError::validation(format!(
                        "{} {k} is stored under another id",
                        stringify!($k)
                    )));
                }
            };
        }
        for_each_collection!(keyed);
        if !self.calendars.contains_key(&self.project.default_calendar) {
            return Err(EngineError::validation("project default calendar missing"));
        }
        let defs: BTreeMap<CalendarId, CalendarDef> = self
            .calendars
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        for id in self.calendars.keys() {
            prswt_calendar::resolve(*id, &defs).map_err(EngineError::from)?;
        }
        self.check_wbs()?;
        for a in self.activities.values() {
            a.validate()?;
            if !self.wbs.contains_key(&a.wbs) {
                return Err(missing("WBS node", a.wbs, &a.code));
            }
            if !self.calendars.contains_key(&a.calendar) {
                return Err(missing("calendar", a.calendar, &a.code));
            }
        }
        self.check_relationships()?;
        for r in self.resources.values() {
            if !self.calendars.contains_key(&r.calendar) {
                return Err(missing("calendar", r.calendar, &r.code));
            }
        }
        for a in self.assignments.values() {
            if !self.activities.contains_key(&a.activity)
                || !self.resources.contains_key(&a.resource)
            {
                return Err(EngineError::validation(format!(
                    "assignment {} references a missing activity or resource",
                    a.id
                )));
            }
            if a.rate_type > 4 {
                return Err(EngineError::validation("rate_type must be 0..=4"));
            }
        }
        for x in self.expenses.values() {
            if !self.activities.contains_key(&x.activity) {
                return Err(missing("activity", x.activity, &x.name));
            }
        }
        self.check_risks()?;
        for m in self.mitigations.values() {
            if !self.risks.contains_key(&m.risk) {
                return Err(missing("risk", m.risk, "mitigation"));
            }
        }
        for g in self.correlations.values() {
            g.validate()?;
            for m in &g.members {
                let ok = match m {
                    UncertainVarRef::ActivityDuration { activity } => {
                        self.activities.contains_key(activity)
                    }
                    UncertainVarRef::RiskOccurrence { risk }
                    | UncertainVarRef::RiskImpact { risk, .. } => self.risks.contains_key(risk),
                };
                if !ok {
                    return Err(EngineError::validation(format!(
                        "correlation group {} references a missing variable",
                        g.id
                    )));
                }
            }
        }
        Ok(())
    }

    fn check_wbs(&self) -> Result<(), EngineError> {
        let mut sibling_codes: HashSet<(Option<WbsId>, &str)> = HashSet::new();
        for w in self.wbs.values() {
            if let Some(p) = w.parent
                && !self.wbs.contains_key(&p)
            {
                return Err(missing("parent WBS", p, &w.code));
            }
            if !sibling_codes.insert((w.parent, w.code.as_str())) {
                return Err(EngineError::validation(format!(
                    "duplicate WBS code {} under the same parent",
                    w.code
                )));
            }
            let mut cur = w.parent;
            let mut steps = 0;
            while let Some(p) = cur {
                if p == w.id || steps > self.wbs.len() {
                    return Err(EngineError::new(
                        ErrorCode::Cycle,
                        format!("WBS {} is its own ancestor", w.code),
                    ));
                }
                cur = self.wbs.get(&p).and_then(|x| x.parent);
                steps += 1;
            }
        }
        Ok(())
    }

    fn check_relationships(&self) -> Result<(), EngineError> {
        let mut seen = HashSet::new();
        let mut adj: HashMap<ActivityId, Vec<ActivityId>> = HashMap::new();
        for r in self.relationships.values() {
            let (Some(p), Some(s)) = (self.activities.get(&r.pred), self.activities.get(&r.succ))
            else {
                return Err(EngineError::validation(format!(
                    "relationship {} references a missing activity",
                    r.id
                )));
            };
            if r.pred == r.succ {
                return Err(EngineError::validation(format!(
                    "activity {} cannot depend on itself",
                    p.code
                )));
            }
            if !seen.insert((r.pred, r.succ, r.kind)) {
                return Err(EngineError::validation(format!(
                    "duplicate {:?} link {} -> {}",
                    r.kind, p.code, s.code
                )));
            }
            adj.entry(r.pred).or_default().push(r.succ);
        }
        // Iterative three-colour DFS for cycles.
        let mut colour: HashMap<ActivityId, u8> = HashMap::new();
        for &start in adj.keys() {
            if colour.get(&start).copied().unwrap_or(0) != 0 {
                continue;
            }
            let mut stack: Vec<(ActivityId, usize)> = vec![(start, 0)];
            colour.insert(start, 1);
            while let Some((v, i)) = stack.last().copied() {
                let next = adj.get(&v).and_then(|n| n.get(i)).copied();
                if let Some(top) = stack.last_mut() {
                    top.1 += 1;
                }
                match next {
                    Some(w) => match colour.get(&w).copied().unwrap_or(0) {
                        0 => {
                            colour.insert(w, 1);
                            stack.push((w, 0));
                        }
                        1 => {
                            let code = self.activities.get(&w).map_or("?", |a| a.code.as_str());
                            return Err(EngineError::new(
                                ErrorCode::Cycle,
                                format!("relationship loop through {code}"),
                            ));
                        }
                        _ => {}
                    },
                    None => {
                        colour.insert(v, 2);
                        stack.pop();
                    }
                }
            }
        }
        Ok(())
    }

    fn check_risks(&self) -> Result<(), EngineError> {
        for r in self.risks.values() {
            r.validate()?;
            for b in &r.bindings {
                let ok = match b {
                    RiskBinding::Activity(a) => self.activities.contains_key(a),
                    RiskBinding::Resource(x) => self.resources.contains_key(x),
                    RiskBinding::WbsBranch(w) => self.wbs.contains_key(w),
                    RiskBinding::Global => true,
                };
                if !ok {
                    return Err(EngineError::validation(format!(
                        "risk {} is bound to a missing entity",
                        r.code
                    )));
                }
            }
            for t in &r.secondary_triggers {
                if !self.risks.contains_key(&t.target) {
                    return Err(missing("risk", t.target, &r.code));
                }
            }
        }
        // Trigger graph must be acyclic.
        let mut indeg: HashMap<RiskId, usize> = self.risks.keys().map(|k| (*k, 0)).collect();
        for r in self.risks.values() {
            for t in &r.secondary_triggers {
                *indeg.entry(t.target).or_default() += 1;
            }
        }
        let mut queue: Vec<RiskId> = indeg.iter().filter(|e| *e.1 == 0).map(|e| *e.0).collect();
        let mut done = 0;
        while let Some(id) = queue.pop() {
            done += 1;
            if let Some(r) = self.risks.get(&id) {
                for t in &r.secondary_triggers {
                    if let Some(d) = indeg.get_mut(&t.target) {
                        *d -= 1;
                        if *d == 0 {
                            queue.push(t.target);
                        }
                    }
                }
            }
        }
        if done != self.risks.len() {
            return Err(EngineError::new(
                ErrorCode::Cycle,
                "secondary risk triggers form a loop",
            ));
        }
        Ok(())
    }
}

fn missing(what: &str, id: impl std::fmt::Display, owner: &str) -> EngineError {
    EngineError::validation(format!("{owner}: {what} {id} does not exist"))
}
