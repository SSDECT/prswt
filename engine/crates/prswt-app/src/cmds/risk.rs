use std::str::FromStr;

use prswt_core::ids::{
    BranchId, CorrelationGroupId, MitigationId, ProjectId, RiskId, SimulationRunId,
};
use prswt_core::model::{
    CorrelationGroup, Estimate, ImpactMode, ImpactProfile, MitigationKind, MitigationPlan,
    MitigationStatus, Proximity, Quantity, Risk, RiskBinding, RiskKind, RiskState, RiskStatus,
    SecondaryTrigger, Subgraph, UncertainVarRef,
};
use prswt_core::{EngineError, Instant, Money, WorkMinutes};
use prswt_cpm::ScheduleInput;
use prswt_risk::mitigate::{Edit, Strategy, propose as propose_mitigation};
use prswt_risk::realize::{RealizationChoice, RealizeInput, plan_realization, schedule_delta};
use prswt_risk::{MitigationMode, SimConfig, SimInput, SimModel, SimResult, Targets, paired_delta};
use prswt_state::{Command, ProjectState};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde_json::{Map, Value, json};

use super::analysis::activity_costs;
use crate::params::{P, days, estimate};
use crate::views::t;
use crate::{Ctx, StoredSim, Workspace};

fn head(w: &Workspace, pid: ProjectId, bid: BranchId) -> Result<&ProjectState, EngineError> {
    Ok(&w.repo_ref(pid)?.branch(bid)?.head)
}

fn mpd(state: &ProjectState) -> i64 {
    state
        .calendars
        .get(&state.project.default_calendar)
        .map_or(480, |c| c.minutes_per_day)
}

pub(crate) fn risk_by(state: &ProjectState, s: &str) -> Result<RiskId, EngineError> {
    if let Ok(id) = s.parse::<RiskId>()
        && state.risks.contains_key(&id)
    {
        return Ok(id);
    }
    state
        .risks
        .values()
        .find(|r| r.code == s)
        .map(|r| r.id)
        .ok_or_else(|| EngineError::not_found(format!("risk `{s}`")))
}

fn quantity(v: &Value) -> Result<Quantity, EngineError> {
    let dec = |x: &Value| -> Result<Decimal, EngineError> {
        match x {
            Value::Number(n) => Decimal::from_str(&n.to_string())
                .map_err(|_| EngineError::validation(format!("bad number {n}"))),
            Value::String(s) => Decimal::from_str(s.trim())
                .map_err(|_| EngineError::validation(format!("bad number {s}"))),
            _ => Err(EngineError::validation("expected a number")),
        }
    };
    match v {
        Value::Object(m) => {
            let p = P(m);
            let g = |k: &str| {
                p.raw(k)
                    .map(dec)
                    .transpose()?
                    .ok_or_else(|| EngineError::validation(format!("missing `{k}`")))
            };
            Ok(match p.opt_str("dist")?.unwrap_or("triangular") {
                "triangular" => Quantity::Triangular {
                    o: g("o")?,
                    m: g("m")?,
                    p: g("p")?,
                },
                "pert" => Quantity::Pert {
                    o: g("o")?,
                    m: g("m")?,
                    p: g("p")?,
                    lambda: 4,
                },
                "uniform" => Quantity::Uniform {
                    min: g("o")?,
                    max: g("p")?,
                },
                d => {
                    return Err(EngineError::validation(format!(
                        "unknown distribution `{d}`"
                    )));
                }
            })
        }
        other => Ok(Quantity::Fixed { value: dec(other)? }),
    }
}

fn impacts(state: &ProjectState, v: Option<&Value>) -> Result<Vec<ImpactProfile>, EngineError> {
    let Some(Value::Array(list)) = v else {
        return Ok(Vec::new());
    };
    let m = mpd(state);
    list.iter()
        .map(|item| {
            let Value::Object(o) = item else {
                return Err(EngineError::validation("each impact must be an object"));
            };
            let p = P(o);
            let mode = if p.bool_or("once", false)? {
                ImpactMode::Once
            } else {
                ImpactMode::EachActivity
            };
            if let Some(x) = p.raw("delay") {
                let delay: Estimate = estimate(x, m)?;
                return Ok(ImpactProfile::ScheduleDelay { delay, mode });
            }
            if let Some(x) = p.raw("duration_factor") {
                return Ok(ImpactProfile::DurationFactor {
                    factor: quantity(x)?,
                    mode,
                });
            }
            if let Some(x) = p.raw("cost") {
                return Ok(ImpactProfile::CostAdd {
                    amount: quantity(x)?,
                    mode,
                });
            }
            if let Some(x) = p.raw("cost_factor") {
                return Ok(ImpactProfile::CostFactor {
                    factor: quantity(x)?,
                    mode,
                });
            }
            if let Some(Value::Object(d)) = p.raw("degrade") {
                let d = P(d);
                let resource = d
                    .opt_str("resource")?
                    .map(|code| {
                        state
                            .resources
                            .values()
                            .find(|r| r.code == code)
                            .map(|r| r.id)
                            .ok_or_else(|| EngineError::not_found(format!("resource `{code}`")))
                    })
                    .transpose()?;
                return Ok(ImpactProfile::ResourceDegradation {
                    resource,
                    factor: quantity(d.raw("factor").unwrap_or(&json!(0.5)))?,
                    window_start: d.instant("from")?,
                    window_end: d.instant("to")?,
                });
            }
            Err(EngineError::validation(
                "impact needs one of delay, duration_factor, cost, cost_factor, degrade",
            ))
        })
        .collect()
}

fn bindings(
    w: &Workspace,
    pid: ProjectId,
    bid: BranchId,
    v: Option<&Value>,
) -> Result<Vec<RiskBinding>, EngineError> {
    let state = head(w, pid, bid)?;
    let Some(Value::Array(list)) = v else {
        return Ok(vec![RiskBinding::Global]);
    };
    list.iter()
        .map(|b| match b {
            Value::String(s) if s == "global" => Ok(RiskBinding::Global),
            Value::String(code) => Ok(RiskBinding::Activity(w.activity_id(pid, bid, code)?)),
            Value::Object(o) => {
                let p = P(o);
                if let Some(code) = p.opt_str("activity")? {
                    return Ok(RiskBinding::Activity(w.activity_id(pid, bid, code)?));
                }
                if let Some(code) = p.opt_str("wbs")? {
                    let id = state
                        .wbs
                        .values()
                        .find(|x| x.code == code)
                        .map(|x| x.id)
                        .ok_or_else(|| EngineError::not_found(format!("WBS `{code}`")))?;
                    return Ok(RiskBinding::WbsBranch(id));
                }
                if let Some(code) = p.opt_str("resource")? {
                    let id = state
                        .resources
                        .values()
                        .find(|x| x.code == code)
                        .map(|x| x.id)
                        .ok_or_else(|| EngineError::not_found(format!("resource `{code}`")))?;
                    return Ok(RiskBinding::Resource(id));
                }
                Err(EngineError::validation(
                    "binding object needs activity, wbs or resource",
                ))
            }
            _ => Err(EngineError::validation(
                "bindings are activity codes, \"global\" or {wbs|resource|activity}",
            )),
        })
        .collect()
}

fn probability(p: &P<'_>, k: &str) -> Result<Option<Decimal>, EngineError> {
    let x = p.opt_decimal(k)?;
    if x.is_some_and(|x| x < Decimal::ZERO || x > Decimal::ONE) {
        return Err(EngineError::validation(format!(
            "`{k}` must be within 0..=1"
        )));
    }
    Ok(x)
}

fn build_risk(
    w: &Workspace,
    pid: ProjectId,
    bid: BranchId,
    p: &P<'_>,
) -> Result<Risk, EngineError> {
    let state = head(w, pid, bid)?;
    let code = p.str("code")?.to_owned();
    if state.risks.values().any(|r| r.code == code) {
        return Err(EngineError::validation(format!(
            "risk `{code}` already exists"
        )));
    }
    let pre = RiskState {
        probability: probability(p, "probability")?
            .ok_or_else(|| EngineError::validation("missing `probability`"))?,
        impacts: impacts(state, p.raw("impacts"))?,
    };
    let post = match p.raw("post") {
        Some(Value::Object(o)) => {
            let q = P(o);
            Some(RiskState {
                probability: probability(&q, "probability")?.unwrap_or(pre.probability),
                impacts: match q.raw("impacts") {
                    Some(x) => impacts(state, Some(x))?,
                    None => pre.impacts.clone(),
                },
            })
        }
        _ => None,
    };
    let proximity = match p.raw("proximity") {
        Some(Value::Object(o)) => {
            let q = P(o);
            Some(Proximity {
                window_start: q.instant("from")?,
                window_end: q.instant("to")?,
            })
        }
        _ => None,
    };
    let kind = match p.opt_str("kind")?.unwrap_or("threat") {
        "threat" => RiskKind::Threat,
        "opportunity" => RiskKind::Opportunity,
        k => return Err(EngineError::validation(format!("unknown risk kind `{k}`"))),
    };
    Ok(Risk {
        id: RiskId::new(),
        title: p.opt_str("title")?.unwrap_or(&code).to_owned(),
        code,
        owner: p.opt_str("owner")?.map(str::to_owned),
        kind,
        status: RiskStatus::Potential,
        pre,
        post,
        bindings: bindings(w, pid, bid, p.raw("bindings"))?,
        proximity,
        correlation_group: None,
        secondary_triggers: Vec::new(),
    })
}

pub fn add(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let r = build_risk(w, pid, bid, &p)?;
    let id = r.id;
    let e = w.apply(
        ctx,
        pid,
        bid,
        vec![Command::UpsertRisk { risk: Box::new(r) }],
        None,
    )?;
    Ok(json!({ "risk": id, "version": e.version }))
}

pub fn inject(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let mut out = add(w, ctx, v)?;
    let p = P::of(v)?;
    if p.bool_or("realize", false)? {
        let mut q: Map<String, Value> = p.0.clone();
        q.insert("risk".into(), json!(p.str("code")?));
        out["realization"] = realize(w, ctx, &Value::Object(q))?;
    }
    Ok(out)
}

pub fn update(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let state = head(w, pid, bid)?;
    let id = risk_by(state, p.str("risk")?)?;
    let mut r = state
        .risks
        .get(&id)
        .cloned()
        .ok_or_else(|| EngineError::not_found("risk"))?;
    if let Some(x) = probability(&p, "probability")? {
        r.pre.probability = x;
    }
    if let Some(x) = probability(&p, "post_probability")? {
        let impacts = r
            .post
            .as_ref()
            .map_or_else(|| r.pre.impacts.clone(), |s| s.impacts.clone());
        r.post = Some(RiskState {
            probability: x,
            impacts,
        });
    }
    if let Some(s) = p.opt_str("status")? {
        r.status = match s {
            "potential" => RiskStatus::Potential,
            "active" => RiskStatus::Active,
            "closed" => RiskStatus::Closed,
            "expired" => RiskStatus::Expired,
            "realized" => RiskStatus::Realized,
            x => return Err(EngineError::validation(format!("unknown status `{x}`"))),
        };
    }
    if p.has("impacts") {
        r.pre.impacts = impacts(state, p.raw("impacts"))?;
    }
    let e = w.apply(
        ctx,
        pid,
        bid,
        vec![Command::UpsertRisk { risk: Box::new(r) }],
        None,
    )?;
    Ok(json!({ "version": e.version }))
}

pub fn list(w: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let state = head(w, pid, bid)?;
    let m = mpd(state);
    let code_of = |b: &RiskBinding| match b {
        RiskBinding::Activity(a) => state
            .activities
            .get(a)
            .map_or("?".into(), |x| x.code.clone()),
        RiskBinding::WbsBranch(wb) => {
            format!("wbs:{}", state.wbs.get(wb).map_or("?", |x| x.code.as_str()))
        }
        RiskBinding::Resource(r) => format!(
            "resource:{}",
            state.resources.get(r).map_or("?", |x| x.code.as_str())
        ),
        RiskBinding::Global => "global".into(),
    };
    let rows: Vec<Value> = state
        .risks
        .values()
        .map(|r| {
            let mean_delay: i64 = r.pre.impacts.iter().map(|i| match i {
                ImpactProfile::ScheduleDelay { delay, .. } => delay.planning_value().0,
                _ => 0,
            }).sum();
            let expected = r.pre.probability * Decimal::from(mean_delay);
            json!({
                "code": r.code,
                "title": r.title,
                "status": r.status,
                "probability": r.pre.probability,
                "post_probability": r.post.as_ref().map(|s| s.probability),
                "bindings": r.bindings.iter().map(code_of).collect::<Vec<_>>(),
                "impacts": r.pre.impacts.len(),
                "expected_delay": days(expected.round().to_i64().unwrap_or(0), m),
                "mitigations": state.mitigations.values().filter(|x| x.risk == r.id)
                    .map(|x| json!({ "kind": x.kind, "status": x.status, "cost": x.cost.to_string() })).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(json!({ "risks": rows }))
}

pub fn realize(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let rid = risk_by(head(w, pid, bid)?, p.str("risk")?)?;
    let choice = match p.opt_str("choice")?.unwrap_or("most_likely") {
        "most_likely" => RealizationChoice::MostLikely,
        "pessimistic" => RealizationChoice::Pessimistic,
        c => {
            return Err(EngineError::validation(format!(
                "choice `{c}`: most_likely|pessimistic"
            )));
        }
    };
    let before = w.schedule(pid, bid)?;
    let costs = super::analysis::cost_rollup(w, pid, bid)?.2.by_activity;
    let state = head(w, pid, bid)?.clone();
    let sn = state.snapshot();
    let at = p.opt_instant("at")?.unwrap_or(state.project.data_date);
    let plan = plan_realization(
        &RealizeInput {
            schedule: &before,
            wbs: &sn.wbs,
            activities: &sn.activities,
            relationships: &sn.relationships,
            assignments: &sn.assignments,
            resources: &sn.resources,
            risks: &sn.risks,
            mitigations: &sn.mitigations,
            activity_costs: &costs,
        },
        rid,
        at,
        &choice,
    )?;
    let m = mpd(&state);
    let mut summary = json!({
        "remaining_changes": plan.remaining_changes.iter().map(|(id, a, b)| json!({
            "activity": state.activities.get(id).map(|x| x.code.clone()),
            "from": days(a.0, m), "to": days(b.0, m),
        })).collect::<Vec<_>>(),
        "expenses": plan.expenses.iter().map(|x| json!({ "name": x.name, "amount": x.budget.to_string() })).collect::<Vec<_>>(),
        "branch_activities_added": plan.add_activities.iter().map(|a| a.code.clone()).collect::<Vec<_>>(),
        "links_removed": plan.remove_relationships.len(),
        "triggered": plan.triggered.iter().map(|(id, pr)| json!({
            "risk": state.risks.get(id).map(|x| x.code.clone()), "probability": pr })).collect::<Vec<_>>(),
    });
    if !p.bool_or("apply", true)? {
        summary["applied"] = json!(false);
        return Ok(summary);
    }
    let mut cmds: Vec<Command> = Vec::new();
    for (id, _, new) in &plan.remaining_changes {
        if let Some(a) = state.activities.get(id) {
            let mut a = a.clone();
            a.remaining = Some(*new);
            cmds.push(Command::UpsertActivity {
                activity: Box::new(a),
            });
        }
    }
    cmds.extend(
        plan.expenses
            .iter()
            .cloned()
            .map(|expense| Command::UpsertExpense { expense }),
    );
    for (rid2, units) in &plan.max_units_changes {
        if let Some(r) = state.resources.get(rid2) {
            let mut r = r.clone();
            r.max_units.clone_from(units);
            cmds.push(Command::UpsertResource { resource: r });
        }
    }
    cmds.extend(
        plan.add_activities
            .iter()
            .cloned()
            .map(|a| Command::UpsertActivity {
                activity: Box::new(a),
            }),
    );
    cmds.extend(
        plan.remove_relationships
            .iter()
            .map(|id| Command::DeleteRelationship { id: *id }),
    );
    cmds.extend(
        plan.add_relationships
            .iter()
            .cloned()
            .map(|relationship| Command::UpsertRelationship { relationship }),
    );
    for (tid, prob) in &plan.triggered {
        if let Some(t) = state.risks.get(tid) {
            let mut t = t.clone();
            t.pre.probability = *prob;
            if t.status == RiskStatus::Potential {
                t.status = RiskStatus::Active;
            }
            cmds.push(Command::UpsertRisk { risk: Box::new(t) });
        }
    }
    cmds.push(Command::SetRiskStatus {
        id: rid,
        status: RiskStatus::Realized,
    });
    for mp in state.mitigations.values().filter(|x| {
        x.risk == rid
            && x.kind == MitigationKind::Contingent
            && x.status == MitigationStatus::Accepted
    }) {
        let mut mp = mp.clone();
        mp.status = MitigationStatus::Executed;
        cmds.push(Command::UpsertMitigation {
            mitigation: Box::new(mp),
        });
    }
    let e = w.apply(ctx, pid, bid, cmds, None)?;
    let after = w.schedule(pid, bid)?;
    let d = schedule_delta(&before, &after, 20);
    summary["applied"] = json!(true);
    summary["version"] = json!(e.version);
    summary["delta"] = json!({
        "finish_before": t(d.finish_before),
        "finish_after": t(d.finish_after),
        "finish_shift": days(d.finish_shift, m),
        "newly_critical": d.newly_critical,
        "no_longer_critical": d.no_longer_critical,
        "float_consumed": d.float_consumed.iter().map(|(c, x)| json!({ "activity": c, "float_lost": days(*x, m) })).collect::<Vec<_>>(),
        "driving_path_before": d.driving_path_before,
        "driving_path_after": d.driving_path_after,
    });
    Ok(summary)
}

pub fn trigger(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let state = head(w, pid, bid)?;
    let id = risk_by(state, p.str("risk")?)?;
    let target = risk_by(state, p.str("target")?)?;
    let cp = probability(&p, "probability")?
        .ok_or_else(|| EngineError::validation("missing `probability`"))?;
    let mut r = state
        .risks
        .get(&id)
        .cloned()
        .ok_or_else(|| EngineError::not_found("risk"))?;
    r.secondary_triggers.retain(|x| x.target != target);
    r.secondary_triggers.push(SecondaryTrigger {
        target,
        conditional_probability: cp,
    });
    let e = w.apply(
        ctx,
        pid,
        bid,
        vec![Command::UpsertRisk { risk: Box::new(r) }],
        None,
    )?;
    Ok(json!({ "version": e.version }))
}

pub fn correlate(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let names: Vec<String> = p.req("members")?;
    if names.len() < 2 || names.len() > 50 {
        return Err(EngineError::validation("correlate 2..=50 members"));
    }
    let rho = p.decimal("rho")?;
    if rho < -Decimal::ONE || rho > Decimal::ONE {
        return Err(EngineError::validation("rho must be within -1..=1"));
    }
    let state = head(w, pid, bid)?;
    let members = names
        .iter()
        .map(|n| match n.strip_prefix("risk:") {
            Some(code) => Ok(UncertainVarRef::RiskOccurrence {
                risk: risk_by(state, code)?,
            }),
            None => Ok(UncertainVarRef::ActivityDuration {
                activity: w.activity_id(pid, bid, n)?,
            }),
        })
        .collect::<Result<Vec<_>, EngineError>>()?;
    let n = members.len();
    let matrix = (0..n * n)
        .map(|k| if k / n == k % n { Decimal::ONE } else { rho })
        .collect();
    let g = CorrelationGroup {
        id: CorrelationGroupId::new(),
        members,
        matrix,
    };
    let id = g.id;
    let e = w.apply(
        ctx,
        pid,
        bid,
        vec![Command::UpsertCorrelation { group: g }],
        None,
    )?;
    Ok(json!({ "group": id, "version": e.version }))
}

fn strategy(
    p: &P<'_>,
    m: i64,
    w: &Workspace,
    pid: ProjectId,
    bid: BranchId,
) -> Result<Strategy, EngineError> {
    let pct = p.opt_decimal("pct")?.unwrap_or(Decimal::from(25));
    let cpd = p.opt_money("cost_per_day")?.unwrap_or(Money::ZERO);
    Ok(match p.str("strategy")? {
        "crash" => Strategy::Crash {
            reduction_pct: pct,
            cost_per_day: cpd,
        },
        "fast_track" => Strategy::FastTrack { overlap_pct: pct },
        "add_resource" => Strategy::AddResource {
            share_pct: pct,
            cost_per_day: cpd,
        },
        "contingency_buffer" => Strategy::ContingencyBuffer {
            duration: p.opt_duration("duration", m)?.unwrap_or(WorkMinutes(5 * m)),
            before: p
                .opt_str("before")?
                .map(|c| w.activity_id(pid, bid, c))
                .transpose()?,
        },
        "alternative_path" => Strategy::AlternativePath {
            duration: p
                .opt_duration("duration", m)?
                .ok_or_else(|| EngineError::validation("alternative_path needs `duration`"))?,
            cost: p.opt_money("cost")?.unwrap_or(Money::ZERO),
        },
        s => return Err(EngineError::validation(format!("unknown strategy `{s}`"))),
    })
}

fn proposal(
    w: &mut Workspace,
    p: &P<'_>,
) -> Result<(ProjectId, BranchId, prswt_risk::mitigate::Proposal), EngineError> {
    let (pid, bid) = w.target(p)?;
    let s = w.schedule(pid, bid)?;
    let state = head(w, pid, bid)?;
    let rid = risk_by(state, p.str("risk")?)?;
    let st = strategy(p, mpd(state), w, pid, bid)?;
    let sn = state.snapshot();
    let risk = state
        .risks
        .get(&rid)
        .ok_or_else(|| EngineError::not_found("risk"))?;
    let prop = propose_mitigation(&s, &sn.activities, &sn.relationships, risk, &st)?;
    Ok((pid, bid, prop))
}

fn describe(state: &ProjectState, prop: &prswt_risk::mitigate::Proposal) -> Value {
    let m = mpd(state);
    let code = |id: prswt_core::ids::ActivityId| {
        state
            .activities
            .get(&id)
            .map_or_else(|| id.to_string(), |a| a.code.clone())
    };
    json!({
        "kind": prop.kind,
        "cost": prop.cost.to_string(),
        "rationale": prop.rationale,
        "edits": prop.edits.iter().map(|e| match e {
            Edit::SetRemaining { activity, from, to } => json!({ "set_remaining": code(*activity), "from": days(from.0, m), "to": days(to.0, m) }),
            Edit::ChangeLink { id, kind, lag } => json!({ "change_link": state.relationships.get(id).map(|r| format!("{}->{}", code(r.pred), code(r.succ))), "kind": format!("{kind:?}"), "lag": days(lag.0, m) }),
            Edit::AddActivity { activity } => json!({ "add_activity": activity.code, "duration": days(activity.duration.planning_value().0, m) }),
            Edit::AddLink { link } => json!({ "add_link": format!("{:?}", link.kind), "pred": link.pred, "succ": link.succ }),
            Edit::RemoveLink { id } => json!({ "remove_link": state.relationships.get(id).map(|r| format!("{}->{}", code(r.pred), code(r.succ))) }),
        }).collect::<Vec<_>>(),
        "branch_activities": prop.subgraph.activities.iter().map(|a| a.code.clone()).collect::<Vec<_>>(),
    })
}

pub fn propose(w: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid, prop) = proposal(w, &p)?;
    Ok(describe(head(w, pid, bid)?, &prop))
}

pub fn attach(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid, prop) = proposal(w, &p)?;
    let state = head(w, pid, bid)?.clone();
    let mut cmds = Vec::new();
    for e in &prop.edits {
        match e {
            Edit::SetRemaining { activity, to, .. } => {
                if let Some(a) = state.activities.get(activity) {
                    let mut a = a.clone();
                    a.remaining = Some(*to);
                    cmds.push(Command::UpsertActivity {
                        activity: Box::new(a),
                    });
                }
            }
            Edit::ChangeLink { id, kind, lag } => {
                if let Some(r) = state.relationships.get(id) {
                    let mut r = r.clone();
                    r.kind = *kind;
                    r.lag = *lag;
                    cmds.push(Command::UpsertRelationship { relationship: r });
                }
            }
            Edit::AddActivity { activity } => cmds.push(Command::UpsertActivity {
                activity: activity.clone(),
            }),
            Edit::AddLink { link } => cmds.push(Command::UpsertRelationship {
                relationship: link.clone(),
            }),
            Edit::RemoveLink { id } => cmds.push(Command::DeleteRelationship { id: *id }),
        }
    }
    let mut risk = state
        .risks
        .get(&prop.risk)
        .cloned()
        .ok_or_else(|| EngineError::not_found("risk"))?;
    if let Some(pp) = probability(&p, "post_probability")? {
        risk.post = Some(RiskState {
            probability: pp,
            impacts: risk.pre.impacts.clone(),
        });
        cmds.push(Command::UpsertRisk {
            risk: Box::new(risk),
        });
    }
    let plan = MitigationPlan {
        id: MitigationId::new(),
        risk: prop.risk,
        kind: prop.kind,
        status: MitigationStatus::Accepted,
        subgraph: if prop.kind == MitigationKind::Contingent {
            prop.subgraph.clone()
        } else {
            Subgraph::default()
        },
        cost: prop.cost,
    };
    let mid = plan.id;
    cmds.push(Command::UpsertMitigation {
        mitigation: Box::new(plan),
    });
    let desc = describe(&state, &prop);
    let e = w.apply(ctx, pid, bid, cmds, None)?;
    Ok(json!({ "mitigation": mid, "version": e.version, "applied": desc }))
}

fn finish_view(r: &SimResult, pc: &prswt_calendar::WorkCalendar, from: Instant) -> Value {
    let inst = |x: f64| Instant(x.round().to_i64().unwrap_or(0));
    let wd = |x: f64| pc.work_between(from, inst(x)).map_or(0, |w| w.0);
    let m = pc.minutes_per_day();
    json!({
        "p10": t(inst(r.finish.p10)), "p50": t(inst(r.finish.p50)),
        "p80": t(inst(r.finish.p80)), "p90": t(inst(r.finish.p90)),
        "mean": t(inst(r.finish.mean)),
        "p50_duration": days(wd(r.finish.p50), m), "p80_duration": days(wd(r.finish.p80), m),
        "std_dev_days": r.finish.std_dev / 1440.0,
    })
}

fn sim_input<'a>(
    state: &'a ProjectState,
    sn: &'a prswt_state::Snapshot,
    costs: &'a std::collections::BTreeMap<prswt_core::ids::ActivityId, prswt_risk::ActivityCost>,
) -> SimInput<'a> {
    SimInput {
        schedule: ScheduleInput {
            project: &state.project,
            calendars: &sn.calendars,
            wbs: &sn.wbs,
            activities: &sn.activities,
            relationships: &sn.relationships,
        },
        risks: &sn.risks,
        mitigations: &sn.mitigations,
        correlations: &sn.correlations,
        assignments: &sn.assignments,
        costs,
    }
}

fn run_branch(
    w: &mut Workspace,
    pid: ProjectId,
    bid: BranchId,
    cfg: &SimConfig,
    mode: MitigationMode,
) -> Result<(SimResult, SimModel, ProjectState), EngineError> {
    let costs = activity_costs(w, pid, bid)?;
    let state = head(w, pid, bid)?.clone();
    let sn = state.snapshot();
    let model = SimModel::compile(&sim_input(&state, &sn, &costs), mode)?;
    let r = model.run(cfg)?;
    Ok((r, model, state))
}

pub fn simulate(w: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let iterations = u32::try_from(
        p.opt_u64("iterations")?
            .unwrap_or(2_000)
            .clamp(100, u64::from(prswt_risk::sim::MAX_ITERATIONS)),
    )
    .unwrap_or(2_000);
    let mode = match p.opt_str("mode")?.unwrap_or("post") {
        "pre" => MitigationMode::Pre,
        "post" => MitigationMode::Post,
        x => return Err(EngineError::validation(format!("mode `{x}`: pre|post"))),
    };
    let finish_by = p.opt_instant("finish_by")?;
    let budget = p.opt_decimal("budget")?.and_then(|d| d.to_f64());
    let cfg = SimConfig {
        iterations,
        seed: p.opt_u64("seed")?.unwrap_or(SimConfig::default().seed),
        mitigation: mode,
        targets: (finish_by.is_some() || budget.is_some()).then_some(Targets { finish_by, budget }),
        ..SimConfig::default()
    };
    let (r, model, state) = run_branch(w, pid, bid, &cfg, mode)?;
    let pc = model.cpm.calendars[model.cpm.project_cal].clone();
    let from = state.project.data_date.max(state.project.planned_start);
    let code = |id: prswt_core::ids::ActivityId| {
        state
            .activities
            .get(&id)
            .map_or_else(|| id.to_string(), |a| a.code.clone())
    };
    let rcode = |id: RiskId| {
        state
            .risks
            .get(&id)
            .map_or_else(|| id.to_string(), |r| r.code.clone())
    };
    let m = pc.minutes_per_day();
    let mut summary = json!({
        "iterations": r.iterations,
        "converged": r.converged,
        "seed": cfg.seed,
        "mode": format!("{mode:?}").to_lowercase(),
        "deterministic_finish": t(w.schedule(pid, bid)?.project_finish()),
        "finish": finish_view(&r, &pc, from),
        "cost": { "p10": r.cost.p10.round(), "p50": r.cost.p50.round(), "p80": r.cost.p80.round(), "p90": r.cost.p90.round(), "mean": r.cost.mean.round() },
        "prob_finish_by": r.prob_finish_by,
        "prob_within_budget": r.prob_within_budget,
        "prob_meet_targets": r.prob_meet_targets,
        "top_criticality": r.criticality.iter().take(10).map(|(id, c)| json!({ "activity": code(*id), "criticality": c })).collect::<Vec<_>>(),
        "top_sensitivity": r.sensitivity.iter().take(10).map(|s| json!({ "activity": code(s.activity), "spearman": s.spearman, "cruciality": s.cruciality })).collect::<Vec<_>>(),
        "risk_tornado": r.risk_tornado.iter().map(|x| json!({
            "risk": rcode(x.risk), "occurrence_rate": x.occurrence_rate,
            "finish_impact_days": x.finish_delta_minutes / 1440.0, "cost_impact": x.cost_delta.round(),
        })).collect::<Vec<_>>(),
        "warnings": model.warnings,
    });
    if let Some(other) = p.opt_str("compare_to")? {
        let mut q = p.0.clone();
        q.insert("branch".into(), json!(other));
        let obid = w.branch_id(pid, &P(&q), "branch")?;
        let (base, _, _) = run_branch(w, pid, obid, &cfg, mode)?;
        let d = paired_delta(&base, &r);
        summary["delta_vs"] = json!({
            "branch": other,
            "finish_days": { "p10": d.finish_minutes.p10 / 1440.0, "p50": d.finish_minutes.p50 / 1440.0, "p80": d.finish_minutes.p80 / 1440.0, "p90": d.finish_minutes.p90 / 1440.0, "mean": d.finish_minutes.mean / 1440.0 },
            "cost": { "p50": d.cost.p50.round(), "p80": d.cost.p80.round(), "mean": d.cost.mean.round() },
            "note": "paired with common random numbers: negative = this branch is earlier/cheaper",
        });
    }
    let _ = m;
    let id = SimulationRunId::new();
    summary["run"] = json!(id);
    w.store_sim(
        id,
        StoredSim {
            project: pid,
            branch: bid,
            result: r,
            summary: summary.clone(),
        },
    );
    Ok(summary)
}

pub fn simulation_get(w: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let id: SimulationRunId = p
        .str("run")?
        .parse()
        .map_err(|_| EngineError::validation("bad run id"))?;
    let include: Vec<String> = p.opt("include")?.unwrap_or_default();
    let (_, sim) = w
        .sims
        .iter()
        .find(|(k, _)| *k == id)
        .ok_or_else(|| EngineError::not_found(format!("simulation run {id}")))?;
    let mut out = sim.summary.clone();
    let state = &w.repo_ref(sim.project)?.branch(sim.branch)?.head;
    let code = |id: prswt_core::ids::ActivityId| {
        state
            .activities
            .get(&id)
            .map_or_else(|| id.to_string(), |a| a.code.clone())
    };
    let r = &sim.result;
    for k in include {
        match k.as_str() {
            "histogram" => out["histogram"] = json!(r.histogram.iter().map(|(a, b, n)| json!({ "from": t(Instant(*a)), "to": t(Instant(*b)), "count": n })).collect::<Vec<_>>()),
            "criticality" => out["criticality"] = json!(r.criticality.iter().map(|(id, c)| json!({ "activity": code(*id), "criticality": c })).collect::<Vec<_>>()),
            "sensitivity" => out["sensitivity"] = json!(r.sensitivity.iter().map(|s| json!({ "activity": code(s.activity), "spearman": s.spearman, "criticality": s.criticality, "cruciality": s.cruciality })).collect::<Vec<_>>()),
            "convergence" => out["convergence"] = json!(r.convergence_trace.iter().map(|(n, a, b)| json!({ "iterations": n, "p50": t(Instant(*a)), "p80": t(Instant(*b)) })).collect::<Vec<_>>()),
            "samples" => out["finish_samples"] = json!(r.finish_samples.iter().take(5000).map(|x| t(Instant(*x))).collect::<Vec<_>>()),
            other => return Err(EngineError::validation(format!("unknown include `{other}`"))),
        }
    }
    Ok(out)
}
