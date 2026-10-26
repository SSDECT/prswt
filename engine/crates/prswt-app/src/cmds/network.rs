use std::collections::BTreeMap;

use chrono::NaiveDate;
use prswt_core::ids::{
    ActivityId, AssignmentId, BranchId, CalendarId, ExpenseId, ProjectId, RelationshipId,
    ResourceId, WbsId,
};
use prswt_core::model::{
    Accrual, Activity, ActivityKind, Assignment, CalendarDef, Constraint, ConstraintKind,
    DayInterval, Effective, Expense, PercentCompleteType, RelationKind, Relationship, Resource,
    ResourceKind, SpreadCurve, WbsNode,
};
use prswt_core::{EngineError, Instant, Money, WorkMinutes};
use prswt_state::{Command, ProjectState};
use rust_decimal::Decimal;
use serde_json::{Value, json};

use crate::params::{P, days};
use crate::{Ctx, Workspace};

fn head(w: &Workspace, pid: ProjectId, bid: BranchId) -> Result<&ProjectState, EngineError> {
    Ok(&w.repo_ref(pid)?.branch(bid)?.head)
}

fn calendar_by(state: &ProjectState, s: Option<&str>) -> Result<CalendarId, EngineError> {
    match s {
        None => Ok(state.project.default_calendar),
        Some(x) => {
            if let Ok(id) = x.parse::<CalendarId>()
                && state.calendars.contains_key(&id)
            {
                return Ok(id);
            }
            state
                .calendars
                .values()
                .find(|c| c.name == x)
                .map(|c| c.id)
                .ok_or_else(|| EngineError::not_found(format!("calendar `{x}`")))
        }
    }
}

fn wbs_by(state: &ProjectState, s: Option<&str>) -> Result<WbsId, EngineError> {
    match s {
        None => state
            .wbs
            .values()
            .find(|w| w.parent.is_none())
            .map(|w| w.id)
            .ok_or_else(|| EngineError::not_found("root WBS")),
        Some(x) => {
            if let Ok(id) = x.parse::<WbsId>()
                && state.wbs.contains_key(&id)
            {
                return Ok(id);
            }
            state
                .wbs
                .values()
                .find(|w| w.code == x)
                .map(|w| w.id)
                .ok_or_else(|| EngineError::not_found(format!("WBS `{x}`")))
        }
    }
}

fn resource_by(state: &ProjectState, s: &str) -> Result<ResourceId, EngineError> {
    if let Ok(id) = s.parse::<ResourceId>()
        && state.resources.contains_key(&id)
    {
        return Ok(id);
    }
    state
        .resources
        .values()
        .find(|r| r.code == s)
        .map(|r| r.id)
        .ok_or_else(|| EngineError::not_found(format!("resource `{s}`")))
}

fn mpd_of(state: &ProjectState, cal: CalendarId) -> i64 {
    state.calendars.get(&cal).map_or(480, |c| c.minutes_per_day)
}

pub(crate) fn parse_kind(s: &str) -> Result<ActivityKind, EngineError> {
    Ok(match s {
        "task" | "task_dependent" => ActivityKind::TaskDependent,
        "resource_dependent" => ActivityKind::ResourceDependent,
        "level_of_effort" | "loe" => ActivityKind::LevelOfEffort,
        "start_milestone" => ActivityKind::StartMilestone,
        "finish_milestone" | "milestone" => ActivityKind::FinishMilestone,
        "wbs_summary" => ActivityKind::WbsSummary,
        _ => {
            return Err(EngineError::validation(format!(
                "unknown activity kind `{s}`"
            )));
        }
    })
}

pub(crate) fn parse_rel(s: &str) -> Result<RelationKind, EngineError> {
    Ok(match s.to_ascii_uppercase().as_str() {
        "FS" => RelationKind::FS,
        "SS" => RelationKind::SS,
        "FF" => RelationKind::FF,
        "SF" => RelationKind::SF,
        _ => {
            return Err(EngineError::validation(format!(
                "unknown relationship type `{s}`"
            )));
        }
    })
}

fn parse_constraint(p: &P<'_>) -> Result<Option<Constraint>, EngineError> {
    let Some(Value::Object(m)) = p.raw("constraint") else {
        return Ok(None);
    };
    let c = P(m);
    let kind = match c.str("kind")? {
        // Short P6 names, and the names the engine itself reports.
        "asap" | "as_soon_as_possible" => ConstraintKind::AsSoonAsPossible,
        "alap" | "as_late_as_possible" => ConstraintKind::AsLateAsPossible,
        "snet" | "start_no_earlier_than" => ConstraintKind::StartNoEarlierThan,
        "snlt" | "start_no_later_than" => ConstraintKind::StartNoLaterThan,
        "fnet" | "finish_no_earlier_than" => ConstraintKind::FinishNoEarlierThan,
        "fnlt" | "finish_no_later_than" => ConstraintKind::FinishNoLaterThan,
        "start_on" => ConstraintKind::StartOn,
        "finish_on" => ConstraintKind::FinishOn,
        "mso" | "mandatory_start" => ConstraintKind::MandatoryStart,
        "mfo" | "mandatory_finish" => ConstraintKind::MandatoryFinish,
        k => return Err(EngineError::validation(format!("unknown constraint `{k}`"))),
    };
    let con = Constraint {
        kind,
        date: c.opt_instant("date")?,
    };
    con.validate()?;
    Ok(Some(con))
}

pub fn calendar_add(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let state = head(w, pid, bid)?;
    let base = p
        .opt_str("base")?
        .map(|b| calendar_by(state, Some(b)))
        .transpose()?;
    let (week, mpd) = match p.opt_str("week")? {
        Some(s) => {
            let (wk, m) = super::project::week_preset(s)?;
            (Some(wk), m)
        }
        None if base.is_some() => (None, base.map_or(480, |b| mpd_of(state, b))),
        None => {
            let (wk, m) = super::project::week_preset("5x8")?;
            (Some(wk), m)
        }
    };
    let mpd = match p.opt_decimal("hours_per_day")? {
        Some(h) => (h * Decimal::from(60)).round().try_into().unwrap_or(mpd),
        None => mpd,
    };
    let cal = CalendarDef {
        id: CalendarId::new(),
        name: p.str("name")?.to_owned(),
        base,
        week,
        exceptions: BTreeMap::new(),
        minutes_per_day: mpd,
    };
    let id = cal.id;
    let e = w.apply(
        ctx,
        pid,
        bid,
        vec![Command::UpsertCalendar { calendar: cal }],
        None,
    )?;
    Ok(json!({ "calendar": id, "version": e.version }))
}

pub fn calendar_exception(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let state = head(w, pid, bid)?;
    let cid = calendar_by(state, p.opt_str("calendar")?)?;
    let mut cal = state
        .calendars
        .get(&cid)
        .cloned()
        .ok_or_else(|| EngineError::not_found("calendar"))?;
    let date = NaiveDate::parse_from_str(p.str("date")?, "%Y-%m-%d")
        .map_err(|_| EngineError::validation("date must be YYYY-MM-DD"))?;
    let work: Vec<(String, String)> = p.opt("work")?.unwrap_or_default();
    let hm = |s: &str| -> Result<u16, EngineError> {
        let (h, m) = s
            .split_once(':')
            .ok_or_else(|| EngineError::validation("times must be HH:MM"))?;
        let (h, m): (u16, u16) = (
            h.parse().map_err(|_| EngineError::validation("bad hour"))?,
            m.parse()
                .map_err(|_| EngineError::validation("bad minute"))?,
        );
        if h > 24 || m > 59 || (h == 24 && m > 0) {
            return Err(EngineError::validation("time out of range"));
        }
        Ok(h * 60 + m)
    };
    let intervals = work
        .iter()
        .map(|(a, b)| Ok(DayInterval::new(hm(a)?, hm(b)?)))
        .collect::<Result<Vec<_>, EngineError>>()?;
    cal.exceptions.insert(date, intervals);
    let e = w.apply(
        ctx,
        pid,
        bid,
        vec![Command::UpsertCalendar { calendar: cal }],
        None,
    )?;
    Ok(json!({ "version": e.version }))
}

pub fn wbs_add(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let state = head(w, pid, bid)?;
    let parent = wbs_by(state, p.opt_str("parent")?)?;
    let node = WbsNode {
        id: WbsId::new(),
        parent: Some(parent),
        code: p.code("code")?.to_owned(),
        name: p.opt_str("name")?.unwrap_or(p.code("code")?).to_owned(),
        obs: None,
        sort_key: i64::try_from(state.wbs.len()).unwrap_or(0),
    };
    let id = node.id;
    let e = w.apply(ctx, pid, bid, vec![Command::UpsertWbs { node }], None)?;
    Ok(json!({ "wbs": id, "version": e.version }))
}

pub fn activity_add(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let state = head(w, pid, bid)?;
    let code = p.code("code")?.to_owned();
    if state.activities.values().any(|a| a.code == code) {
        return Err(EngineError::validation(format!(
            "activity code `{code}` already exists"
        )));
    }
    let calendar = calendar_by(state, p.opt_str("calendar")?)?;
    let mpd = mpd_of(state, calendar);
    let kind = p
        .opt_str("kind")?
        .map(parse_kind)
        .transpose()?
        .unwrap_or(ActivityKind::TaskDependent);
    let duration = match p.opt_estimate("duration", mpd)? {
        Some(d) => d,
        None if kind.is_milestone() => prswt_core::model::Estimate::fixed(WorkMinutes::ZERO),
        None => {
            return Err(EngineError::validation(
                "missing required parameter `duration`",
            ));
        }
    };
    let a = Activity {
        id: ActivityId::new(),
        name: p.opt_str("name")?.unwrap_or(&code).to_owned(),
        code,
        wbs: wbs_by(state, p.opt_str("wbs")?)?,
        kind,
        calendar,
        duration,
        remaining: None,
        primary_constraint: parse_constraint(&p)?,
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
    let mut cmds = vec![Command::UpsertActivity {
        activity: Box::new(a),
    }];
    let preds: Vec<String> = p.opt("predecessors")?.unwrap_or_default();
    for pred in preds {
        cmds.push(Command::UpsertRelationship {
            relationship: Relationship {
                id: RelationshipId::new(),
                pred: w.activity_id(pid, bid, &pred)?,
                succ: id,
                kind: RelationKind::FS,
                lag: WorkMinutes::ZERO,
                lag_calendar: None,
            },
        });
    }
    let e = w.apply(ctx, pid, bid, cmds, p.opt_u64("expected_version")?)?;
    Ok(json!({ "activity": id, "version": e.version }))
}

pub fn activity_update(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let id = w.activity_id(pid, bid, p.str("activity")?)?;
    let state = head(w, pid, bid)?;
    let mut a = state
        .activities
        .get(&id)
        .cloned()
        .ok_or_else(|| EngineError::not_found("activity"))?;
    if let Some(n) = p.opt_str("name")? {
        n.clone_into(&mut a.name);
    }
    if let Some(k) = p.opt_str("kind")? {
        a.kind = parse_kind(k)?;
    }
    if let Some(c) = p.opt_str("calendar")? {
        a.calendar = calendar_by(state, Some(c))?;
    }
    if let Some(wb) = p.opt_str("wbs")? {
        a.wbs = wbs_by(state, Some(wb))?;
    }
    let mpd = mpd_of(state, a.calendar);
    if let Some(d) = p.opt_estimate("duration", mpd)? {
        a.duration = d;
        a.remaining = None;
    }
    if p.bool_or("clear_constraint", false)? {
        a.primary_constraint = None;
        a.secondary_constraint = None;
    }
    if let Some(c) = parse_constraint(&p)? {
        a.primary_constraint = Some(c);
    }
    let e = w.apply(
        ctx,
        pid,
        bid,
        vec![Command::UpsertActivity {
            activity: Box::new(a),
        }],
        p.opt_u64("expected_version")?,
    )?;
    Ok(json!({ "activity": id, "version": e.version }))
}

pub fn activity_delete(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let id = w.activity_id(pid, bid, p.str("activity")?)?;
    let e = w.apply(
        ctx,
        pid,
        bid,
        vec![Command::DeleteActivity {
            id,
            reconnect: p.bool_or("reconnect", false)?,
        }],
        None,
    )?;
    Ok(json!({ "version": e.version, "changes": e.changes.len() }))
}

pub fn activity_progress(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let id = w.activity_id(pid, bid, p.str("activity")?)?;
    let state = head(w, pid, bid)?;
    let mpd = state
        .activities
        .get(&id)
        .map_or(480, |a| mpd_of(state, a.calendar));
    let pct = p.opt_decimal("pct")?;
    if pct.is_some_and(|x| x < Decimal::ZERO || x > Decimal::ONE_HUNDRED) {
        return Err(EngineError::validation("pct must be 0..=100"));
    }
    let cmd = Command::UpdateProgress {
        activity: id,
        actual_start: p.opt_instant("actual_start")?,
        actual_finish: p.opt_instant("actual_finish")?,
        pct_complete: pct,
        remaining: p.opt_duration("remaining", mpd)?,
        expected_finish: p.opt_instant("expected_finish")?,
        suspend: p.opt_instant("suspend")?,
        resume: p.opt_instant("resume")?,
        clear_expected_finish: p.bool_or("clear_expected_finish", false)?,
        clear_suspend: p.bool_or("clear_suspend", false)?,
    };
    check_suspend(state, id, &cmd)?;
    let mut cmds = vec![cmd];
    if let Some(ac) = p.opt_money("actual_cost")? {
        cmds.extend(split_actual_cost(state, id, ac)?);
    }
    let e = w.apply(ctx, pid, bid, cmds, None)?;
    Ok(json!({ "version": e.version }))
}

/// P6 rules for suspend/resume: only started, unfinished work can be
/// suspended; suspend is on or after the actual start; resume follows suspend.
fn check_suspend(state: &ProjectState, id: ActivityId, cmd: &Command) -> Result<(), EngineError> {
    let Command::UpdateProgress {
        actual_start,
        actual_finish,
        suspend,
        resume,
        clear_suspend,
        ..
    } = cmd
    else {
        return Ok(());
    };
    let a = state
        .activities
        .get(&id)
        .ok_or_else(|| EngineError::not_found(format!("activity {id}")))?;
    let keep =
        |new: &Option<prswt_core::Instant>, old| if *clear_suspend { *new } else { new.or(old) };
    let start = actual_start.or(a.actual_start);
    let finish = actual_finish.or(a.actual_finish);
    let (sus, res) = (keep(suspend, a.suspend), keep(resume, a.resume));
    if res.is_some() && sus.is_none() {
        return Err(EngineError::validation("resume needs a suspend date"));
    }
    let Some(sus) = sus else { return Ok(()) };
    match start {
        None => {
            return Err(EngineError::validation(
                "only a started activity can be suspended",
            ));
        }
        Some(st) if sus < st => {
            return Err(EngineError::validation(
                "suspend cannot be before the actual start",
            ));
        }
        Some(_) => {}
    }
    if finish.is_some() && (suspend.is_some() || resume.is_some()) {
        return Err(EngineError::validation(
            "a finished activity cannot be suspended",
        ));
    }
    if res.is_some_and(|r| r < sus) {
        return Err(EngineError::validation("resume cannot be before suspend"));
    }
    Ok(())
}

/// Spreads an activity's cost to date over its assignments and expenses in
/// proportion to their planned cost (equally when nothing has a planned
/// cost), so EVM's AC sums back to exactly `total`. The last item takes the
/// rounding remainder.
fn split_actual_cost(
    state: &ProjectState,
    activity: ActivityId,
    total: Money,
) -> Result<Vec<Command>, EngineError> {
    if total.0 < Decimal::ZERO {
        return Err(EngineError::validation("actual_cost must be 0 or more"));
    }
    let assignments: Vec<&Assignment> = state
        .assignments
        .values()
        .filter(|a| a.activity == activity)
        .collect();
    let expenses: Vec<&Expense> = state
        .expenses
        .values()
        .filter(|x| x.activity == activity)
        .collect();
    let planned = |a: &Assignment| -> Decimal {
        state
            .resources
            .get(&a.resource)
            .and_then(|r| r.rates.first())
            .and_then(|e| e.value.get(usize::from(a.rate_type)))
            .and_then(|rate| rate.0.checked_mul(a.budget_units))
            .unwrap_or(Decimal::ZERO)
    };
    let mut weights: Vec<Decimal> = assignments.iter().map(|a| planned(a)).collect();
    weights.extend(expenses.iter().map(|x| x.budget.0));
    if weights.is_empty() {
        return Err(EngineError::validation(
            "the activity has no resources or expenses to carry an actual cost; assign a resource or add an expense first",
        ));
    }
    let sum = weights
        .iter()
        .try_fold(Decimal::ZERO, |s, w| s.checked_add(*w))
        .ok_or_else(|| EngineError::validation("planned cost is too large"))?;
    let sum = if sum <= Decimal::ZERO {
        weights.fill(Decimal::ONE);
        Decimal::from(weights.len())
    } else {
        sum
    };
    let mut shares = Vec::with_capacity(weights.len());
    let mut given = Decimal::ZERO;
    for (i, w) in weights.iter().enumerate() {
        let share = if i + 1 == weights.len() {
            total.0 - given
        } else {
            total
                .0
                .checked_mul(*w)
                .and_then(|x| x.checked_div(sum))
                .map(|x| x.round_dp(2))
                .ok_or_else(|| EngineError::validation("actual_cost is too large"))?
        };
        given += share;
        shares.push(Money(share));
    }
    let mut out = Vec::with_capacity(shares.len());
    for (a, share) in assignments.iter().zip(&shares) {
        let mut a = (*a).clone();
        a.actual_cost = *share;
        out.push(Command::UpsertAssignment { assignment: a });
    }
    for (x, share) in expenses.iter().zip(shares.iter().skip(assignments.len())) {
        let mut x = (*x).clone();
        x.actual = *share;
        out.push(Command::UpsertExpense { expense: x });
    }
    Ok(out)
}

pub fn relationship_add(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let pred = w.activity_id(pid, bid, p.str("pred")?)?;
    let succ = w.activity_id(pid, bid, p.str("succ")?)?;
    let state = head(w, pid, bid)?;
    let mpd = state
        .activities
        .get(&pred)
        .map_or(480, |a| mpd_of(state, a.calendar));
    let kind = parse_rel(p.opt_str("kind")?.unwrap_or("FS"))?;
    let r = Relationship {
        id: RelationshipId::new(),
        pred,
        succ,
        kind,
        lag: p.opt_duration("lag", mpd)?.unwrap_or(WorkMinutes::ZERO),
        lag_calendar: None,
    };
    let id = r.id;
    let e = w.apply(
        ctx,
        pid,
        bid,
        vec![Command::UpsertRelationship { relationship: r }],
        None,
    )?;
    Ok(json!({ "relationship": id, "version": e.version }))
}

pub fn relationship_delete(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let pred = w.activity_id(pid, bid, p.str("pred")?)?;
    let succ = w.activity_id(pid, bid, p.str("succ")?)?;
    let kind = p.opt_str("kind")?.map(parse_rel).transpose()?;
    let state = head(w, pid, bid)?;
    let ids: Vec<RelationshipId> = state
        .relationships
        .values()
        .filter(|r| r.pred == pred && r.succ == succ && kind.is_none_or(|k| k == r.kind))
        .map(|r| r.id)
        .collect();
    if ids.is_empty() {
        return Err(EngineError::not_found("relationship"));
    }
    let cmds = ids
        .into_iter()
        .map(|id| Command::DeleteRelationship { id })
        .collect();
    let e = w.apply(ctx, pid, bid, cmds, None)?;
    Ok(json!({ "version": e.version }))
}

pub fn resource_add(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let state = head(w, pid, bid)?;
    let code = p.code("code")?.to_owned();
    if state.resources.values().any(|r| r.code == code) {
        return Err(EngineError::validation(format!(
            "resource `{code}` already exists"
        )));
    }
    let kind = match p.opt_str("kind")?.unwrap_or("labor") {
        "labor" => ResourceKind::Labor,
        "non_labor" | "equipment" => ResourceKind::NonLabor,
        "material" => ResourceKind::Material,
        k => {
            return Err(EngineError::validation(format!(
                "unknown resource kind `{k}`"
            )));
        }
    };
    let rate = p.opt_money("rate")?.unwrap_or(Money::ZERO);
    let r = Resource {
        id: ResourceId::new(),
        name: p.opt_str("name")?.unwrap_or(&code).to_owned(),
        code,
        kind,
        calendar: calendar_by(state, p.opt_str("calendar")?)?,
        max_units: vec![Effective {
            from: Instant::MIN,
            value: p.opt_decimal("max_units")?.unwrap_or(Decimal::ONE),
        }],
        rates: vec![Effective {
            from: Instant::MIN,
            value: [rate; 5],
        }],
        unit_of_measure: p.opt_str("unit")?.map(str::to_owned),
        obs: None,
    };
    let id = r.id;
    let e = w.apply(
        ctx,
        pid,
        bid,
        vec![Command::UpsertResource { resource: r }],
        None,
    )?;
    Ok(json!({ "resource": id, "version": e.version }))
}

pub fn resource_assign(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let activity = w.activity_id(pid, bid, p.str("activity")?)?;
    let state = head(w, pid, bid)?;
    let resource = resource_by(state, p.str("resource")?)?;
    let uph = p.opt_decimal("units_per_hour")?.unwrap_or(Decimal::ONE);
    let hours = state.activities.get(&activity).map_or(Decimal::ZERO, |a| {
        Decimal::from(a.duration.planning_value().0) / Decimal::from(60)
    });
    let units = p.opt_decimal("units")?.unwrap_or(hours * uph);
    let curve = match p.opt_str("curve")?.unwrap_or("uniform") {
        "uniform" => SpreadCurve::Uniform,
        "front" | "front_loaded" => SpreadCurve::FrontLoaded,
        "back" | "back_loaded" => SpreadCurve::BackLoaded,
        "bell" => SpreadCurve::Bell,
        c => return Err(EngineError::validation(format!("unknown curve `{c}`"))),
    };
    let a = Assignment {
        id: AssignmentId::new(),
        activity,
        resource,
        budget_units: units,
        units_per_hour: uph,
        rate_type: 0,
        cost_account: None,
        curve,
        actual_units: Decimal::ZERO,
        actual_cost: Money::ZERO,
        remaining_units: None,
    };
    let id = a.id;
    let e = w.apply(
        ctx,
        pid,
        bid,
        vec![Command::UpsertAssignment { assignment: a }],
        None,
    )?;
    Ok(json!({ "assignment": id, "units": units, "version": e.version }))
}

pub fn expense_add(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let activity = w.activity_id(pid, bid, p.str("activity")?)?;
    let accrual = match p.opt_str("accrual")?.unwrap_or("uniform") {
        "start" => Accrual::Start,
        "end" => Accrual::End,
        "uniform" => Accrual::Uniform,
        a => return Err(EngineError::validation(format!("unknown accrual `{a}`"))),
    };
    let x = Expense {
        id: ExpenseId::new(),
        activity,
        cost_account: None,
        name: p.str("name")?.to_owned(),
        budget: Money(p.decimal("amount")?),
        actual: p.opt_money("actual")?.unwrap_or(Money::ZERO),
        accrual,
    };
    let id = x.id;
    let e = w.apply(
        ctx,
        pid,
        bid,
        vec![Command::UpsertExpense { expense: x }],
        None,
    )?;
    Ok(json!({ "expense": id, "version": e.version }))
}

#[allow(dead_code)]
pub(crate) fn fmt_days(w: i64, mpd: i64) -> String {
    days(w, mpd)
}
