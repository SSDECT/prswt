use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use prswt_core::ids::{ActivityId, BaselineId, BranchId, ProjectId};
use prswt_core::model::BaselineMeta;
use prswt_core::{EngineError, Instant, Money};
use prswt_cpm::Schedule;
use prswt_cpm::dcma::{DcmaContext, assess};
use prswt_evm::{Baseline, EacMethod, evaluate};
use prswt_resource::{
    CostRollup, LevelOptions, Period, ResourceContext, ResourceInput, level as level_resources,
};
use prswt_risk::ActivityCost;
use prswt_state::{Command, Snapshot};
use serde_json::{Value, json};

use crate::params::{P, days};
use crate::views::{activity_row, summary, t};
use crate::{Ctx, Workspace};

pub(crate) fn snap(w: &Workspace, pid: ProjectId, bid: BranchId) -> Result<Snapshot, EngineError> {
    Ok(w.repo_ref(pid)?.branch(bid)?.head.snapshot())
}

pub(crate) fn cost_rollup(
    w: &mut Workspace,
    pid: ProjectId,
    bid: BranchId,
) -> Result<(Arc<Schedule>, Snapshot, CostRollup), EngineError> {
    let s = w.schedule(pid, bid)?;
    let sn = snap(w, pid, bid)?;
    let roll = {
        let ctx = ResourceContext::new(ResourceInput {
            schedule: &s,
            resources: &sn.resources,
            assignments: &sn.assignments,
            expenses: &sn.expenses,
            calendars: &sn.calendars,
        })?;
        ctx.cost_rollup(&sn.wbs)?
    };
    Ok((s, sn, roll))
}

/// Split of planned cost: resource cost scales with duration, expenses don't.
pub(crate) fn activity_costs(
    w: &mut Workspace,
    pid: ProjectId,
    bid: BranchId,
) -> Result<BTreeMap<ActivityId, ActivityCost>, EngineError> {
    use rust_decimal::prelude::ToPrimitive;
    let (_, sn, roll) = cost_rollup(w, pid, bid)?;
    let mut fixed: HashMap<ActivityId, f64> = HashMap::new();
    for x in &sn.expenses {
        *fixed.entry(x.activity).or_default() += x.budget.0.to_f64().unwrap_or(0.0);
    }
    Ok(roll
        .by_activity
        .iter()
        .map(|(id, m)| {
            let total = m.0.to_f64().unwrap_or(0.0);
            let f = fixed.get(id).copied().unwrap_or(0.0);
            (
                *id,
                ActivityCost {
                    time_dependent: (total - f).max(0.0),
                    fixed: f,
                },
            )
        })
        .collect())
}

pub fn run(w: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let s = w.schedule(pid, bid)?;
    let state = &w.repo_ref(pid)?.branch(bid)?.head;
    let mut out = summary(state, &s);
    let paths = s.float_paths(1);
    out["critical_path"] = json!(
        paths
            .first()
            .map(|p| p
                .activities
                .iter()
                .map(|&i| s.model.nodes[i].code.clone())
                .collect::<Vec<_>>())
            .unwrap_or_default()
    );
    out["out_of_sequence"] = out_of_sequence(&s);
    Ok(out)
}

/// CPM-19 rows for reports: `{pred, succ, kind}` by activity code.
fn out_of_sequence(s: &Schedule) -> Value {
    json!(
        s.out_of_sequence()
            .iter()
            .map(|o| json!({
                "pred": s.model.nodes[o.pred].code,
                "succ": s.model.nodes[o.succ].code,
                "kind": o.kind,
            }))
            .collect::<Vec<_>>()
    )
}

pub fn activities(w: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let s = w.schedule(pid, bid)?;
    // A baseline that cannot be rebuilt only drops the comparison columns.
    let baseline = baseline_dates(w, pid, bid).ok().flatten();
    let with_links = p.bool_or("links", false)?;
    let state = &w.repo_ref(pid)?.branch(bid)?.head;
    let only_crit = p.bool_or("critical", false)?;
    let text = p.opt_str("text")?.map(str::to_lowercase);
    let wbs = p.opt_str("wbs")?;
    let mut idx: Vec<usize> = (0..s.model.nodes.len())
        .filter(|&i| {
            let n = &s.model.nodes[i];
            let a = state.activities.get(&n.id);
            (!only_crit || s.critical[i])
                && text.as_ref().is_none_or(|q| {
                    n.code.to_lowercase().contains(q.as_str())
                        || a.is_some_and(|a| a.name.to_lowercase().contains(q.as_str()))
                })
                && wbs.is_none_or(|code| {
                    a.and_then(|a| state.wbs.get(&a.wbs))
                        .is_some_and(|w| w.code == code)
                })
        })
        .collect();
    if let Some(mf) = p.opt_duration("max_float", s.model.calendars[0].minutes_per_day())? {
        idx.retain(|&i| s.dates.tf[i].is_some_and(|tf| tf <= mf.0));
    }
    match p.opt_str("sort")?.unwrap_or("start") {
        "float" => idx.sort_by_key(|&i| (s.dates.tf[i].unwrap_or(i64::MAX), s.dates.es[i])),
        "code" => idx.sort_by(|&a, &b| s.model.nodes[a].code.cmp(&s.model.nodes[b].code)),
        _ => idx.sort_by_key(|&i| (s.dates.es[i], s.dates.ef[i])),
    }
    let total = idx.len();
    let offset = usize::try_from(p.opt_u64("offset")?.unwrap_or(0)).unwrap_or(0);
    let limit = usize::try_from(p.opt_u64("limit")?.unwrap_or(200).min(1000)).unwrap_or(200);
    let rows: Vec<Value> = idx
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|i| {
            let mut row = activity_row(state, &s, i);
            let n = &s.model.nodes[i];
            if let Some((_, dates)) = &baseline
                && let Some(&(bs, bf)) = dates.get(&n.id)
            {
                let cal = s.model.cal(i);
                let variance = cal
                    .work_between(bf, s.dates.ef[i])
                    .map(|w| days(w.0, cal.minutes_per_day()))
                    .ok();
                row["baseline_start"] = json!(t(bs));
                row["baseline_finish"] = json!(t(bf));
                row["finish_variance"] = json!(variance);
            }
            if with_links {
                row["predecessors"] = json!(
                    s.model
                        .in_edges(i)
                        .iter()
                        .map(|&e| {
                            let edge = &s.model.edges[e];
                            json!({
                                "activity": s.model.nodes[edge.pred].code,
                                "type": edge.kind,
                                "driving": s.dates.edge_slack[e] == Some(0),
                            })
                        })
                        .collect::<Vec<_>>()
                );
            }
            row
        })
        .collect();
    Ok(json!({
        "total": total,
        "offset": offset,
        "baseline": baseline.map(|b| b.0),
        "items": rows,
    }))
}

/// Early dates of every activity in the latest baseline (`None` without one).
fn baseline_dates(
    w: &Workspace,
    pid: ProjectId,
    bid: BranchId,
) -> Result<Option<(String, HashMap<ActivityId, (Instant, Instant)>)>, EngineError> {
    let head = &w.repo_ref(pid)?.branch(bid)?.head;
    let Some(meta) = head.baselines.values().max_by_key(|b| b.captured_at) else {
        return Ok(None);
    };
    let state = w.repo_ref(pid)?.state_at(bid, meta.branch_version)?;
    let sn = state.snapshot();
    let s = prswt_cpm::schedule(&prswt_cpm::ScheduleInput {
        project: &state.project,
        calendars: &sn.calendars,
        wbs: &sn.wbs,
        activities: &sn.activities,
        relationships: &sn.relationships,
    })?;
    let dates = s
        .model
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id, (s.dates.es[i], s.dates.ef[i])))
        .collect();
    Ok(Some((meta.name.clone(), dates)))
}

pub fn activity(w: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let id = w.activity_id(pid, bid, p.str("activity")?)?;
    let s = w.schedule(pid, bid)?;
    let state = &w.repo_ref(pid)?.branch(bid)?.head;
    let i = s
        .index_of(id)
        .ok_or_else(|| EngineError::not_found("activity"))?;
    let mpd = s.model.cal(i).minutes_per_day();
    let link = |e: usize, other: usize| {
        let edge = &s.model.edges[e];
        json!({
            "activity": s.model.nodes[other].code,
            "type": format!("{:?}", edge.kind),
            "lag": days(edge.lag, mpd),
            "slack": s.dates.edge_slack[e].map(|x| days(x, mpd)),
            "driving": s.dates.edge_slack[e] == Some(0),
        })
    };
    let preds: Vec<Value> = s
        .model
        .in_edges(i)
        .iter()
        .map(|&e| link(e, s.model.edges[e].pred))
        .collect();
    let succs: Vec<Value> = s
        .model
        .out_edges(i)
        .iter()
        .map(|&e| link(e, s.model.edges[e].succ))
        .collect();
    let risks: Vec<Value> = state
        .risks
        .values()
        .filter(|r| r.bindings.contains(&prswt_core::model::RiskBinding::Activity(id)))
        .map(|r| json!({ "code": r.code, "title": r.title, "probability": r.pre.probability, "status": r.status }))
        .collect();
    let assignments: Vec<Value> = state
        .assignments
        .values()
        .filter(|a| a.activity == id)
        .map(|a| json!({ "resource": state.resources.get(&a.resource).map(|r| r.code.clone()), "units": a.budget_units }))
        .collect();
    let mut row = activity_row(state, &s, i);
    row["predecessors"] = json!(preds);
    row["successors"] = json!(succs);
    row["risks"] = json!(risks);
    row["assignments"] = json!(assignments);
    row["explain"] = explain_chain(&s, i);
    Ok(row)
}

pub(crate) fn explain_chain(s: &Schedule, i: usize) -> Value {
    json!(s
        .explain(i)
        .iter()
        .map(|st| {
            let n = &s.model.nodes[st.activity];
            let why = match st.driver {
                prswt_cpm::Driver::Link(e) => format!(
                    "{:?} from {}",
                    s.model.edges[e].kind, s.model.nodes[s.model.edges[e].pred].code
                ),
                d => format!("{d:?}").to_lowercase(),
            };
            json!({ "activity": n.code, "early_start": t(s.dates.es[st.activity]), "driven_by": why })
        })
        .collect::<Vec<_>>())
}

pub fn paths(w: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let k = usize::try_from(p.opt_u64("k")?.unwrap_or(3).clamp(1, 50)).unwrap_or(3);
    let s = w.schedule(pid, bid)?;
    let mpd = s.model.calendars[s.model.project_cal].minutes_per_day();
    let out: Vec<Value> = s
        .float_paths(k)
        .iter()
        .enumerate()
        .map(|(n, fp)| {
            json!({
                "path": n + 1,
                "float": days(fp.float, mpd),
                "activities": fp.activities.iter().map(|&i| json!({
                    "code": s.model.nodes[i].code,
                    "early_start": t(s.dates.es[i]),
                    "early_finish": t(s.dates.ef[i]),
                    "total_float": s.dates.tf[i].map(|x| days(x, mpd)),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(json!({ "finish": t(s.project_finish()), "paths": out }))
}

pub fn health(w: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let s = w.schedule(pid, bid)?;
    let sn = snap(w, pid, bid)?;
    let resourced: HashSet<ActivityId> = sn
        .assignments
        .iter()
        .map(|a| a.activity)
        .chain(sn.expenses.iter().map(|x| x.activity))
        .collect();
    let report = assess(
        &s,
        &DcmaContext {
            resourced,
            baseline_finish: None,
        },
    );
    let mut out = serde_json::to_value(report).unwrap_or(Value::Null);
    if let Some(obj) = out.as_object_mut() {
        obj.insert("out_of_sequence".into(), out_of_sequence(&s));
    }
    Ok(out)
}

fn res_ctx<'a>(s: &'a Schedule, sn: &'a Snapshot) -> Result<ResourceContext<'a>, EngineError> {
    ResourceContext::new(ResourceInput {
        schedule: s,
        resources: &sn.resources,
        assignments: &sn.assignments,
        expenses: &sn.expenses,
        calendars: &sn.calendars,
    })
}

pub fn histogram(w: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let period = match p.opt_str("period")?.unwrap_or("week") {
        "day" => Period::Day,
        "week" => Period::Week,
        x => return Err(EngineError::validation(format!("period `{x}`: day|week"))),
    };
    let only = p.opt_str("resource")?;
    let s = w.schedule(pid, bid)?;
    let sn = snap(w, pid, bid)?;
    let ctx = res_ctx(&s, &sn)?;
    let h = ctx.histogram(period)?;
    let out: Vec<Value> = h
        .iter()
        .filter_map(|(rid, buckets)| {
            let r = ctx.resource(*rid)?;
            if only.is_some_and(|o| o != r.code) {
                return None;
            }
            Some(json!({
                "resource": r.code,
                "buckets": buckets.iter().map(|(k, u)| {
                    let day = match period { Period::Day => *k, Period::Week => k * 7 + 2 };
                    json!({ "from": prswt_core::Instant::from_day_index(day).date().map(|d| d.to_string()), "units": u })
                }).collect::<Vec<_>>(),
            }))
        })
        .collect();
    Ok(json!({ "period": p.opt_str("period")?.unwrap_or("week"), "resources": out }))
}

pub fn overallocation(w: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let s = w.schedule(pid, bid)?;
    let sn = snap(w, pid, bid)?;
    let ctx = res_ctx(&s, &sn)?;
    let day = |d: i64| {
        prswt_core::Instant::from_day_index(d)
            .date()
            .map(|x| x.to_string())
    };
    let items: Vec<Value> = ctx
        .overallocations()?
        .iter()
        .map(|o| {
            json!({
                "resource": ctx.resource(o.resource).map(|r| r.code.clone()),
                "from": day(o.from_day),
                "to": day(o.to_day),
                "peak_required": o.peak_required,
                "available": o.available,
            })
        })
        .collect();
    Ok(json!({ "overallocations": items }))
}

pub fn level(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    if p.bool_or("clear", false)? {
        return clear_leveling(w, ctx, pid, bid);
    }
    // Level from the unleveled network (P6 levels from early dates), so a
    // second run replaces the first instead of stacking delays on it.
    let mut sn = snap(w, pid, bid)?;
    for a in &mut sn.activities {
        a.leveled_start = None;
    }
    let project = w.repo_ref(pid)?.branch(bid)?.head.project.clone();
    let s = prswt_cpm::schedule(&prswt_cpm::ScheduleInput {
        project: &project,
        calendars: &sn.calendars,
        wbs: &sn.wbs,
        activities: &sn.activities,
        relationships: &sn.relationships,
    })?;
    let only: Vec<String> = p.opt("resources")?.unwrap_or_default();
    let rctx = res_ctx(&s, &sn)?;
    let opts = LevelOptions {
        preserve_float: p.bool_or("preserve_float", false)?,
        resources: sn
            .resources
            .iter()
            .filter(|r| only.contains(&r.code))
            .map(|r| r.id)
            .collect(),
        ..LevelOptions::default()
    };
    let r = level_resources(&rctx, &opts)?;
    let delayed: Vec<Value> = r
        .delayed
        .iter()
        .filter_map(|(id, m)| {
            let i = s.index_of(*id)?;
            Some(json!({ "activity": s.model.nodes[i].code, "delay": days(m.0, s.model.cal(i).minutes_per_day()), "new_start": t(r.start[i]) }))
        })
        .collect();
    let mut out = json!({
        "finish_before": t(s.project_finish()),
        "finish_after": t(r.finish_date),
        "delayed": delayed,
        "unresolved": r.unresolved.iter().filter_map(|id| s.index_of(*id).map(|i| s.model.nodes[i].code.clone())).collect::<Vec<_>>(),
    });
    if p.bool_or("apply", false)? {
        // Leveled starts are kept apart from constraints (P6 leveled dates):
        // set on the delayed activities, removed from the rest.
        let state = &w.repo_ref(pid)?.branch(bid)?.head;
        // Only resourced work keeps a leveled start; activities pushed by
        // logic alone follow their predecessors on the next calculation.
        let resourced: HashSet<ActivityId> = sn.assignments.iter().map(|a| a.activity).collect();
        let delayed: HashMap<ActivityId, Instant> = r
            .delayed
            .iter()
            .filter(|(id, _)| resourced.contains(id))
            .filter_map(|(id, _)| Some((*id, r.start[s.index_of(*id)?])))
            .collect();
        let cmds: Vec<Command> = state
            .activities
            .values()
            .filter(|a| a.leveled_start != delayed.get(&a.id).copied())
            .map(|a| {
                let mut a = a.clone();
                a.leveled_start = delayed.get(&a.id).copied();
                Command::UpsertActivity {
                    activity: Box::new(a),
                }
            })
            .collect();
        if !cmds.is_empty() {
            let e = w.apply(ctx, pid, bid, cmds, None)?;
            out["version"] = json!(e.version);
        }
        out["applied"] = json!(true);
    }
    Ok(out)
}

/// Removes every leveled start (P6 "clear leveling").
fn clear_leveling(
    w: &mut Workspace,
    ctx: &Ctx,
    pid: ProjectId,
    bid: BranchId,
) -> Result<Value, EngineError> {
    let state = &w.repo_ref(pid)?.branch(bid)?.head;
    let cmds: Vec<Command> = state
        .activities
        .values()
        .filter(|a| a.leveled_start.is_some())
        .map(|a| {
            let mut a = a.clone();
            a.leveled_start = None;
            Command::UpsertActivity {
                activity: Box::new(a),
            }
        })
        .collect();
    let cleared = cmds.len();
    let mut out = json!({ "cleared": cleared });
    if cleared > 0 {
        let e = w.apply(ctx, pid, bid, cmds, None)?;
        out["version"] = json!(e.version);
    }
    Ok(out)
}

pub fn cost(w: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let (_, sn, roll) = cost_rollup(w, pid, bid)?;
    let wbs: Vec<Value> = sn
        .wbs
        .iter()
        .map(|n| json!({ "wbs": n.code, "name": n.name, "budget": roll.by_wbs.get(&n.id).copied().unwrap_or(Money::ZERO).to_string() }))
        .collect();
    Ok(json!({ "total": roll.total.to_string(), "by_wbs": wbs }))
}

pub fn baseline(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let name = p.str("name")?.to_owned();
    capture(w, ctx, pid, bid, &name)
}

pub(crate) fn capture(
    w: &mut Workspace,
    ctx: &Ctx,
    pid: ProjectId,
    bid: BranchId,
    name: &str,
) -> Result<Value, EngineError> {
    let (s, _, roll) = cost_rollup(w, pid, bid)?;
    let id = BaselineId::new();
    let bl = Baseline::capture(id, name, &s, &roll);
    let version = w.repo_ref(pid)?.branch(bid)?.meta.version;
    let meta = BaselineMeta {
        id,
        name: name.to_owned(),
        branch_version: version,
        captured_at: w.now(),
    };
    let e = w.apply(
        ctx,
        pid,
        bid,
        vec![Command::CaptureBaseline { baseline: meta }],
        None,
    )?;
    let bac = bl.bac;
    w.baselines.insert(id, Arc::new(bl));
    Ok(
        json!({ "baseline": id, "name": name, "bac": bac.to_string(), "finish": t(s.project_finish()), "version": e.version }),
    )
}

/// Recomputes a baseline's time-phased budget from the state at its version
/// (baselines are derived data; only their metadata is persisted).
fn rebuild_baseline(
    w: &Workspace,
    pid: ProjectId,
    bid: BranchId,
    meta: &BaselineMeta,
) -> Result<Arc<Baseline>, EngineError> {
    let state = w.repo_ref(pid)?.state_at(bid, meta.branch_version)?;
    let sn = state.snapshot();
    let s = prswt_cpm::schedule(&prswt_cpm::ScheduleInput {
        project: &state.project,
        calendars: &sn.calendars,
        wbs: &sn.wbs,
        activities: &sn.activities,
        relationships: &sn.relationships,
    })?;
    let roll = res_ctx(&s, &sn)?.cost_rollup(&sn.wbs)?;
    Ok(Arc::new(Baseline::capture(
        meta.id,
        meta.name.clone(),
        &s,
        &roll,
    )))
}

pub fn evm(w: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let (pid, bid) = w.target(&p)?;
    let state = &w.repo_ref(pid)?.branch(bid)?.head;
    let wanted = p.opt_str("baseline")?;
    let meta = state
        .baselines
        .values()
        .filter(|b| wanted.is_none_or(|x| b.name == x || b.id.to_string() == x))
        .max_by_key(|b| b.captured_at)
        .cloned()
        .ok_or_else(|| EngineError::not_found("baseline (capture one with `baseline.capture`)"))?;
    let bl = match w.baselines.get(&meta.id).cloned() {
        Some(b) => b,
        None => {
            let rebuilt = rebuild_baseline(w, pid, bid, &meta)?;
            w.baselines.insert(meta.id, rebuilt.clone());
            rebuilt
        }
    };
    let state = &w.repo_ref(pid)?.branch(bid)?.head;
    let as_of = p.opt_instant("as_of")?.unwrap_or(state.project.data_date);
    let method = match p.opt_str("method")?.unwrap_or("cpi") {
        "cpi" => EacMethod::Cpi,
        "remaining_at_budget" => EacMethod::RemainingAtBudget,
        "cpi_spi" => EacMethod::CpiSpi,
        m => return Err(EngineError::validation(format!("unknown EAC method `{m}`"))),
    };
    let sn = state.snapshot();
    let r = evaluate(
        &bl,
        as_of,
        &sn.activities,
        &sn.assignments,
        &sn.expenses,
        method,
    );
    let mut out = serde_json::to_value(&r).unwrap_or(Value::Null);
    out["baseline"] = json!(meta.name);
    out["as_of"] = json!(t(as_of));
    Ok(out)
}
