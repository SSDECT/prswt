//! RSK-15: rule-based mitigation proposals (`generate_mitigation_subgraph`).
//!
//! Proposals are deterministic and *not applied*: an agent or human reviews
//! the edits, tests them in a sandbox with `run_impact_simulation`, and only
//! then accepts. Strategies (see `vault/02-Architecture/04-MCP-Spec.md` §2.5):
//! crash, fast-track, add-resource, contingency buffer, alternative path.

use std::collections::HashSet;

use prswt_core::ids::{ActivityId, RelationshipId, RiskId};
use prswt_core::model::{
    Activity, ActivityKind, Anchor, Estimate, MitigationKind, RelationKind, Relationship, Risk,
    RiskBinding, Subgraph,
};
use prswt_core::{EngineError, Money, WorkMinutes};
use prswt_cpm::Schedule;
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use serde::{Deserialize, Serialize};

/// How to mitigate a risk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "strategy")]
pub enum Strategy {
    /// Shorten bound critical activities by `reduction_pct` at `cost_per_day`.
    Crash {
        /// Share of duration removed (0..=100).
        reduction_pct: Decimal,
        /// Cost per working day saved.
        cost_per_day: Money,
    },
    /// Turn FS links into bound activities into SS with `overlap_pct` overlap.
    FastTrack {
        /// Overlap as a share of the predecessor's duration (0..=100).
        overlap_pct: Decimal,
    },
    /// Move `share_pct` of each bound critical activity to a parallel helper.
    AddResource {
        /// Share of work moved to the helper (0..=100).
        share_pct: Decimal,
        /// Cost per day of moved work.
        cost_per_day: Money,
    },
    /// Schedule reserve of `duration` in front of `before` (default: last activity).
    ContingencyBuffer {
        /// Buffer length.
        duration: WorkMinutes,
        /// Activity to protect; default the last activity.
        before: Option<ActivityId>,
    },
    /// Contingent fallback: an alternative activity replacing the bound
    /// activity's outgoing links when the risk occurs.
    AlternativePath {
        /// Duration of the alternative activity.
        duration: WorkMinutes,
        /// Cost if the fallback is used.
        cost: Money,
    },
}

/// One proposed network change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "edit")]
pub enum Edit {
    /// Change an activity's remaining duration.
    SetRemaining {
        /// Activity.
        activity: ActivityId,
        /// Current value.
        from: WorkMinutes,
        /// Proposed value.
        to: WorkMinutes,
    },
    /// Change a link's type and lag.
    ChangeLink {
        /// Link.
        id: RelationshipId,
        /// New type.
        kind: RelationKind,
        /// New lag.
        lag: WorkMinutes,
    },
    /// Add an activity.
    AddActivity {
        /// The activity.
        activity: Box<Activity>,
    },
    /// Add a link.
    AddLink {
        /// The link.
        link: Relationship,
    },
    /// Remove a link.
    RemoveLink {
        /// The link.
        id: RelationshipId,
    },
}

/// A mitigation proposal for review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Proposal {
    /// Risk it mitigates.
    pub risk: RiskId,
    /// Preventive (edits applied now) or contingent (dormant branch).
    pub kind: MitigationKind,
    /// Direct network edits (preventive strategies).
    pub edits: Vec<Edit>,
    /// Dormant branch (contingent strategies).
    pub subgraph: Subgraph,
    /// Estimated cost.
    pub cost: Money,
    /// Why these edits.
    pub rationale: String,
}

/// Builds a proposal for `risk` with `strategy`; nothing is applied.
pub fn propose(
    s: &Schedule,
    activities: &[Activity],
    relationships: &[Relationship],
    risk: &Risk,
    strategy: &Strategy,
) -> Result<Proposal, EngineError> {
    let bound: Vec<ActivityId> = risk
        .bindings
        .iter()
        .filter_map(|b| match b {
            RiskBinding::Activity(id) => Some(*id),
            _ => None,
        })
        .filter(|id| s.index_of(*id).is_some())
        .collect();
    let critical_bound: Vec<ActivityId> = {
        let c: Vec<ActivityId> = bound
            .iter()
            .copied()
            .filter(|id| s.index_of(*id).is_some_and(|i| s.critical[i]))
            .collect();
        if c.is_empty() { bound.clone() } else { c }
    };
    let rem = |id: ActivityId| s.index_of(id).map_or(0, |i| s.model.nodes[i].dur);
    let mpd = |id: ActivityId| {
        s.index_of(id)
            .map_or(480, |i| s.model.cal(i).minutes_per_day())
    };
    let find = |id: ActivityId| activities.iter().find(|a| a.id == id);
    let mut p = Proposal {
        risk: risk.id,
        kind: MitigationKind::Preventive,
        edits: Vec::new(),
        subgraph: Subgraph::default(),
        cost: Money::ZERO,
        rationale: String::new(),
    };
    match strategy {
        Strategy::Crash {
            reduction_pct,
            cost_per_day,
        } => {
            let keep = Decimal::ONE - pct(*reduction_pct)?;
            for id in &critical_bound {
                let from = rem(*id);
                let to = scale(from, keep);
                p.cost += *cost_per_day * days(from - to, mpd(*id));
                p.edits.push(Edit::SetRemaining {
                    activity: *id,
                    from: WorkMinutes(from),
                    to: WorkMinutes(to),
                });
            }
            p.rationale = format!(
                "crash {} critical activit{} by {reduction_pct}%",
                critical_bound.len(),
                if critical_bound.len() == 1 {
                    "y"
                } else {
                    "ies"
                }
            );
        }
        Strategy::FastTrack { overlap_pct } => {
            let overlap = pct(*overlap_pct)?;
            let targets: HashSet<_> = bound.iter().collect();
            for r in relationships
                .iter()
                .filter(|r| r.kind == RelationKind::FS && targets.contains(&r.succ))
            {
                let pred_dur = rem(r.pred);
                let lag = scale(pred_dur, Decimal::ONE - overlap) + r.lag.0;
                p.edits.push(Edit::ChangeLink {
                    id: r.id,
                    kind: RelationKind::SS,
                    lag: WorkMinutes(lag),
                });
            }
            p.rationale = format!("overlap {} FS link(s) by {overlap_pct}%", p.edits.len());
        }
        Strategy::AddResource {
            share_pct,
            cost_per_day,
        } => {
            let share = pct(*share_pct)?;
            for id in &critical_bound {
                let Some(a) = find(*id) else { continue };
                let from = rem(*id);
                let moved = scale(from, share);
                p.edits.push(Edit::SetRemaining {
                    activity: *id,
                    from: WorkMinutes(from),
                    to: WorkMinutes(from - moved),
                });
                let helper = derived(a, "ASSIST", WorkMinutes(moved));
                p.cost += *cost_per_day * days(moved, mpd(*id));
                p.edits.push(Edit::AddLink {
                    link: link(a.id, helper.id, RelationKind::SS, 0),
                });
                for r in relationships.iter().filter(|r| r.pred == a.id) {
                    p.edits.push(Edit::AddLink {
                        link: link(helper.id, r.succ, RelationKind::FS, 0),
                    });
                }
                p.edits.push(Edit::AddActivity {
                    activity: Box::new(helper),
                });
            }
            p.rationale = format!("split {share_pct}% of work to parallel helpers");
        }
        Strategy::ContingencyBuffer { duration, before } => {
            let target = before
                .or_else(|| {
                    s.model
                        .topo
                        .iter()
                        .rev()
                        .map(|&i| &s.model.nodes[i])
                        .find(|n| n.is_driving())
                        .map(|n| n.id)
                })
                .ok_or_else(|| EngineError::validation("no activity to protect with a buffer"))?;
            let Some(anchor) = find(target) else {
                return Err(EngineError::not_found(format!("activity {target}")));
            };
            let buffer = derived(anchor, "BUFFER", *duration);
            for r in relationships.iter().filter(|r| r.succ == target) {
                p.edits.push(Edit::RemoveLink { id: r.id });
                p.edits.push(Edit::AddLink {
                    link: Relationship {
                        succ: buffer.id,
                        id: RelationshipId::new(),
                        ..r.clone()
                    },
                });
            }
            p.edits.push(Edit::AddLink {
                link: link(buffer.id, target, RelationKind::FS, 0),
            });
            p.edits.push(Edit::AddActivity {
                activity: Box::new(buffer),
            });
            p.rationale = format!("schedule reserve before {}", anchor.code);
        }
        Strategy::AlternativePath { duration, cost } => {
            p.kind = MitigationKind::Contingent;
            p.cost = *cost;
            for id in &bound {
                let Some(a) = find(*id) else { continue };
                let alt = derived(a, "ALT", *duration);
                for r in relationships.iter().filter(|r| r.succ == a.id) {
                    p.subgraph.anchors.push(Anchor {
                        pred: r.pred,
                        succ: alt.id,
                        kind: r.kind,
                        lag: r.lag,
                        replaces: None,
                    });
                }
                for r in relationships.iter().filter(|r| r.pred == a.id) {
                    p.subgraph.anchors.push(Anchor {
                        pred: alt.id,
                        succ: r.succ,
                        kind: r.kind,
                        lag: r.lag,
                        replaces: Some(r.id),
                    });
                }
                p.subgraph.activities.push(alt);
            }
            p.rationale = "fallback branch activated only if the risk occurs".into();
        }
    }
    Ok(p)
}

fn pct(x: Decimal) -> Result<Decimal, EngineError> {
    if x < Decimal::ZERO || x > Decimal::ONE_HUNDRED {
        return Err(EngineError::validation(
            "percentages must be within 0..=100",
        ));
    }
    Ok(x / Decimal::ONE_HUNDRED)
}

fn scale(minutes: i64, f: Decimal) -> i64 {
    (Decimal::from(minutes) * f)
        .round()
        .to_i64()
        .unwrap_or(minutes)
}

fn days(minutes: i64, minutes_per_day: i64) -> Decimal {
    Decimal::from(minutes) / Decimal::from(minutes_per_day.max(1))
}

fn derived(a: &Activity, suffix: &str, duration: WorkMinutes) -> Activity {
    Activity {
        id: ActivityId::new(),
        code: format!("{}-{suffix}", a.code),
        name: format!("{} ({})", a.name, suffix.to_lowercase()),
        kind: ActivityKind::TaskDependent,
        duration: Estimate::fixed(duration),
        remaining: None,
        primary_constraint: None,
        secondary_constraint: None,
        actual_start: None,
        actual_finish: None,
        pct_complete: Decimal::ZERO,
        ..a.clone()
    }
}

fn link(pred: ActivityId, succ: ActivityId, kind: RelationKind, lag: i64) -> Relationship {
    Relationship {
        id: RelationshipId::new(),
        pred,
        succ,
        kind,
        lag: WorkMinutes(lag),
        lag_calendar: None,
    }
}
