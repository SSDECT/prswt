use prswt_core::{EngineError, ErrorCode};
use prswt_risk::realize::schedule_delta;
use prswt_state::{BranchKind, BranchStatus, MergeStrategy};
use serde_json::{Value, json};

use crate::params::{P, days};
use crate::views::t;
use crate::{AgentPolicy, Ctx, Workspace};

/// Longest sandbox lifetime an agent may request (7 days).
pub(crate) const MAX_TTL_MINUTES: u64 = 7 * 24 * 60;

pub fn create(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let pid = w.project_id(&p)?;
    let from = w.branch_id(pid, &p, "from")?;
    let name = p.str("name")?.to_owned();
    if w.repo_ref(pid)?.find_by_name(&name).is_some() || name == "live" {
        return Err(EngineError::validation(format!(
            "an open branch named `{name}` exists"
        )));
    }
    let ttl = p.opt_u64("ttl_minutes")?.map(|x| x.min(MAX_TTL_MINUTES));
    let at = w.now();
    let id = w.repo_mut(pid)?.fork(
        from,
        &name,
        ctx.author.clone(),
        at,
        ttl.and_then(|x| i64::try_from(x).ok()),
    )?;
    let version = w.repo_ref(pid)?.branch(from)?.meta.version;
    Ok(json!({ "sandbox": name, "id": id, "fork_version": version }))
}

pub fn list(w: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let pid = w.project_id(&p)?;
    let now = w.now();
    let repo = w.repo_mut(pid)?;
    repo.expire(now);
    let rows: Vec<Value> = repo
        .branches()
        .map(|b| {
            json!({
                "name": b.meta.name,
                "id": b.meta.id,
                "kind": b.meta.kind,
                "status": b.meta.status,
                "version": b.meta.version,
                "fork_version": b.meta.fork_version,
                "created_by": b.meta.created_by,
                "created_at": t(b.meta.created_at),
                "ttl_minutes": b.meta.ttl_minutes,
                "proposal": b.meta.proposal.as_ref().map(|p| json!({
                    "by": p.by, "note": p.note, "at": t(p.at),
                })),
            })
        })
        .collect();
    Ok(json!({ "branches": rows }))
}

pub fn diff(w: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let pid = w.project_id(&p)?;
    let base = w.branch_id(pid, &p, "base")?;
    let head = w.branch_id(pid, &p, "head")?;
    let changes = w.repo_ref(pid)?.diff(base, head)?;
    let sb = w.schedule(pid, base)?;
    let sh = w.schedule(pid, head)?;
    let d = schedule_delta(&sb, &sh, 20);
    let m = sh.model.calendars[sh.model.project_cal].minutes_per_day();
    let state = &w.repo_ref(pid)?.branch(head)?.head;
    let label = |c: &prswt_state::Change| -> String {
        let code = c
            .after
            .as_ref()
            .or(c.before.as_ref())
            .and_then(|v| v.get("code"))
            .and_then(Value::as_str);
        format!("{:?} {}", c.entity.kind, code.unwrap_or(&c.entity.id)).to_lowercase()
    };
    let _ = state;
    Ok(json!({
        "changes": changes.iter().map(|c| json!({
            "entity": label(c),
            "change": match (&c.before, &c.after) { (None, Some(_)) => "added", (Some(_), None) => "removed", _ => "modified" },
            "fields": c.fields,
        })).collect::<Vec<_>>(),
        "schedule": {
            "finish_base": t(d.finish_before),
            "finish_head": t(d.finish_after),
            "finish_shift": days(d.finish_shift, m),
            "newly_critical": d.newly_critical,
            "no_longer_critical": d.no_longer_critical,
            "driving_path_base": d.driving_path_before,
            "driving_path_head": d.driving_path_after,
        },
    }))
}

pub fn commit(w: &mut Workspace, ctx: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let pid = w.project_id(&p)?;
    let sb = w.branch_id(pid, &p, "sandbox")?;
    if w.repo_ref(pid)?.branch(sb)?.meta.kind == BranchKind::Live {
        return Err(EngineError::validation(
            "`sandbox` must name a sandbox, not live",
        ));
    }
    if ctx.is_agent() && w.policy == AgentPolicy::RequireHuman {
        return Err(EngineError::new(
            ErrorCode::LiveWriteForbidden,
            "committing to live needs human approval: show the user `sandbox.diff` and ask them to run `sandbox.commit`",
        ));
    }
    let strategy = match p.opt_str("strategy")?.unwrap_or("reject_on_conflict") {
        "reject_on_conflict" => MergeStrategy::RejectOnConflict,
        "prefer_sandbox" => MergeStrategy::PreferSandbox,
        "prefer_live" => MergeStrategy::PreferLive,
        s => return Err(EngineError::validation(format!("unknown strategy `{s}`"))),
    };
    let at = w.now();
    let (event, conflicts) = w
        .repo_mut(pid)?
        .merge(sb, strategy, ctx.author.clone(), at)?;
    w.emit(pid, &event);
    let live = w.repo_ref(pid)?.live;
    let mut out = json!({
        "live_version": event.version,
        "changes": event.changes.len(),
        "conflicts_resolved": conflicts.len(),
    });
    if let Some(name) = p.opt_str("capture_baseline")? {
        out["baseline"] = super::analysis::capture(w, ctx, pid, live, name)?;
    }
    Ok(out)
}

pub fn discard(w: &mut Workspace, _: &Ctx, v: &Value) -> Result<Value, EngineError> {
    let p = P::of(v)?;
    let pid = w.project_id(&p)?;
    let sb = w.branch_id(pid, &p, "sandbox")?;
    w.repo_mut(pid)?.discard(sb)?;
    Ok(json!({ "discarded": true, "status": BranchStatus::Discarded }))
}
