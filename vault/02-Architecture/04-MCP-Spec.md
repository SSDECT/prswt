---
type: spec
section: SPEC-04
status: draft
updated: 2026-10-23
---
# SPEC-04 — MCP Specification

Prev: [[03-Pipeline]] · Next: [[Scorecard]]

Server name `prswt`. Transports: stdio, streamable HTTP. Every tool has a JSON Schema input, returns `structuredContent` (typed output schema) plus a short text summary for the LLM.

## 0. Conventions
- `branch` defaults to `"live"`. Every **write** tool accepts `expected_version?: u64` (optimistic concurrency) and returns a `WriteOutcome`.
- Durations in tool I/O are strings with units: `"5d"`, `"3h"`, `"-2d"`, converted with the activity's or project's hours_per_day. Dates use ISO-8601 local time.
- Errors: `{ code, message, details }`. Codes: `NOT_FOUND`, `VALIDATION`, `CYCLE`, `VERSION_CONFLICT`, `MERGE_CONFLICT`, `RISK_NOT_REALIZABLE`, `LIVE_WRITE_FORBIDDEN` (agents with the policy flag may write only in sandboxes), `LIMIT`.
- Large outputs are paginated (`cursor`, `limit` ≤ 200) and ranked by relevance (e.g. by float ascending).

```ts
type WriteOutcome = {
  branch: string; version: number;
  warnings: Issue[];
  delta: ScheduleDelta;            // computed after recalculation
}
type ScheduleDelta = {
  finish: { before: string; after: string; days: number };
  cost:   { before: string; after: string };
  newly_critical: Ref[]; no_longer_critical: Ref[];
  float_consumed: { ref: Ref; days: number }[];   // top 20
  new_overallocations: { resource: Ref; periods: number }[];
}
type Ref = { id: string; code: string; name: string };
```

## 1. Resources (read-only semantic context)
| URI template | Content |
|---|---|
| `prswt://projects` | projects with live finish, cost, health flags |
| `prswt://projects/{p}/branches` | live + sandboxes (owner, fork version, age, status) |
| `prswt://projects/{p}/b/{b}/summary` | data date, finish (det. + last P50/P80/P90), BAC/EAC, CPI/SPI, # critical, top 5 risks, DCMA score |
| `…/critical-path` | ordered chains of activities and driving links with dates, TF, calendars |
| `…/near-critical{?threshold,k}` | top-K near-critical chains with path float |
| `…/wbs` | WBS tree with rollups (dates, cost, % complete, risk exposure) |
| `…/activities/{a}` | full activity context: estimate, dates, floats, preds/succs with link slack, driving predecessor, assignments, bound risks, criticality index |
| `…/resources/contention{?period}` | heatmap matrix: resource × period → utilisation %, overallocated cells |
| `…/evm{?as_of}` | PV/EV/AC/indices/forecasts + time series |
| `…/risks` | register: pre/post probability × impact, EMV, status, bindings, mitigations |
| `…/simulations/{run}` | quantiles, histogram, criticality, tornado, convergence |
| `…/diff/{other}` | structured diff between branches (entities + schedule delta) |
| `…/events{?since}` | audit log |
| `…/health` | DCMA-14 check results with offending activities |

## 2. Tools
### 2.1 Introspection
| Tool | Input | Output |
|---|---|---|
| `list_projects` | `{}` | `{ projects: ProjectSummary[] }` |
| `get_schedule_summary` | `{ project, branch? }` | `Summary` (as in the resource) |
| `query_activities` | `{ project, branch?, filter?: { wbs?, text?, critical?, max_float?: Dur, kind?, has_risk?, resource? }, sort?: "float"\|"start"\|"code", cursor?, limit? }` | `{ items: ActivityRow[], next? }` |
| `explain_activity_dates` | `{ project, branch?, activity }` | `{ start_driver: Chain, finish_driver: Chain, constraints_applied, calendar_effects }`: why it starts and finishes when it does |
| `schedule_health_check` | `{ project, branch? }` | `{ score, checks: { id, name, threshold, value, pass, offenders: Ref[] }[] }` (DCMA-14: logic, leads, lags, rel. types, hard constraints, high float, negative float, high duration, invalid dates, resources, missed tasks, CP test, CPLI, BEI) |

### 2.2 Editing (atomic and composite)
| Tool | Input (essential fields) |
|---|---|
| `create_project` | `{ name, start, calendar?: CalendarSpec, settings? }` |
| `upsert_wbs_node` | `{ project, branch?, id?, parent?, code, name }` |
| `upsert_activity` | `{ project, branch?, id?, code, name, wbs, kind?, calendar?, duration: Dur \| ThreePoint, constraint? }` |
| `delete_activity` | `{ project, branch?, activity, reconnect?: bool }` (reconnect: wire preds to succs) |
| `link_activities` / `unlink_activities` | `{ project, branch?, pred, succ, kind: "FS"\|"SS"\|"FF"\|"SF", lag?: Dur }` |
| `set_estimate` | `{ project, branch?, activity, estimate: { o, m, p, dist: "triangular"\|"pert"\|"uniform" } \| Dur }` |
| `update_progress` | `{ project, branch?, data_date?, items: { activity, actual_start?, actual_finish?, pct?, remaining? }[] }` |
| `upsert_calendar` | `{ project, id?, base?, week?, exceptions? }` |
| `upsert_resource` / `assign_resource` | resource def / `{ activity, resource, units_per_hour, budget_units?, rate_type?, cost_account? }` |
| `apply_commands` | `{ project, branch?, commands: Command[], expected_version? }`: atomic batch of any of the above |

All return `WriteOutcome`.

### 2.3 Scheduling & resources
| Tool | Input | Output |
|---|---|---|
| `recalculate_schedule` | `{ project, branch? }` | `{ version, summary, issues }` |
| `detect_overallocation` | `{ project, branch?, resource?, period? }` | `{ items: { resource, from, to, required, available }[] }` |
| `auto_level_resources` | `{ project, branch, resources?: Ref[], priority?: ("float"\|"start"\|"code")[], preserve_float?: bool }` | `WriteOutcome & { moved: { activity, days }[] }` (rejected on live unless permitted) |

### 2.4 Baselines & EVM
| Tool | Input | Output |
|---|---|---|
| `capture_baseline` | `{ project, branch?, name }` | `{ baseline_id, bac }` |
| `compute_evm` | `{ project, branch?, baseline?, as_of?, eac_method? }` | `EvmReport` |

### 2.5 Risk
| Tool | Input | Output |
|---|---|---|
| `upsert_risk` | `{ project, branch?, id?, title, kind, probability, impacts: Impact[], bindings: Binding[], proximity?, post?: { probability, impacts } }` | `WriteOutcome & { risk_id }` |
| `inject_risk_event` | `{ project, branch, title, probability?, impacts, bindings, realize?: bool }`: create + bind (+ realize) in one call, sandbox-oriented | `WriteOutcome & { risk_id }` |
| `realize_risk` | `{ project, branch, risk, at?, realization?: "most_likely"\|"pessimistic"\|{ values } }` | `WriteOutcome & { triggered: Ref[] }` |
| `set_correlation` | `{ project, branch?, group, members: VarRef[], matrix: number[][] }` | `WriteOutcome & { repaired: bool }` |
| `generate_mitigation_subgraph` | `{ project, branch, risk, strategy: "crash"\|"fast_track"\|"add_resource"\|"contingency_buffer"\|"alternative_path", params }` | `{ proposal: Subgraph, estimated_effect }`: deterministic, rule-based proposal, **not** applied |
| `attach_mitigation` | `{ project, branch, risk, kind: "preventive"\|"contingent", subgraph, cost, accept?: bool }` | `WriteOutcome & { mitigation_id }` |

Strategies in `generate_mitigation_subgraph`:
- crash: shorten the critical activities bound to the risk, at a cost per day.
- fast_track: convert FS links into SS links with a lag at given overlap %.
- add_resource: a parallel activity that takes work off the bound activities.
- contingency_buffer: a buffer activity before the protected milestone.
- alternative_path: a contingent branch that replaces the anchored links.

### 2.6 Simulation
| Tool | Input | Output |
|---|---|---|
| `run_impact_simulation` | `{ project, branch, iterations?: 1000..100000, seed?, mitigation_mode?: "pre"\|"post"\|"both", confidence?: number[], targets?: { finish_by?, budget? }, compare_to?: branch, milestones?: Ref[] }` | `{ run_id, finish: Quantiles, cost: Quantiles, prob_meet_targets, top_criticality: [...], tornado_duration: [...], tornado_risks: [...], delta_vs?: { finish: Quantiles, cost: Quantiles }, converged, iterations_run }` |
| `get_simulation_result` | `{ run_id, include?: ("histogram"\|"cdf"\|"criticality_all"\|"joint")[] }` | full result |

### 2.7 Sandboxes
| Tool | Input | Output |
|---|---|---|
| `create_sandbox` | `{ project, from?: branch, name, ttl_minutes? }` | `{ branch, fork_version }` |
| `list_sandboxes` | `{ project }` | `Branch[]` |
| `diff_branches` | `{ project, base: branch, head: branch }` | `{ entity_changes: Change[], schedule: ScheduleDelta, evm_delta, risk_delta }` |
| `commit_sandbox_to_baseline` | `{ project, sandbox, strategy?: "reject_on_conflict"\|"prefer_sandbox"\|"prefer_live", capture_baseline?: string }` | `{ live_version, conflicts?: Conflict[], baseline_id? }` |
| `discard_sandbox` | `{ project, sandbox }` | `{ discarded: true }` |

## 3. Prompts (agent workflow templates)
| Prompt | Arguments | Workflow it instructs |
|---|---|---|
| `risk_auditor` | project, branch?, depth? | 1. `schedule_health_check` 2. read `critical-path` + `near-critical` 3. `run_impact_simulation` 4. pick top-5 tornado drivers and fragile paths (TF < 5d with CI > 30 %) 5. for each: sandbox + `generate_mitigation_subgraph` + simulate with `compare_to: live` 6. report hidden fragility ranked by ΔP80 per cost |
| `supply_chain_correlator` | project, event_description, affected_suppliers? | map the external event to resources/activities (`query_activities` by resource or text) → sandbox → `inject_risk_event` with calibrated delay distribution → simulate → propose graph updates + mitigations; commit only on human approval |
| `executive_briefing` | project, as_of?, audience? ("board"\|"pmo") | summary + EVM + latest simulation → one-page brief: status RAG, P50/P80 finish vs target, cost forecast, top 3 risks, decisions needed |
| `mitigation_optimizer` | project, risk or "top_n", budget | iterate strategies in separate sandboxes, compare with common random numbers, choose the Pareto set (ΔP80 vs cost) |
| `schedule_repair` | project | from the DCMA failures, propose logic fixes in a sandbox (missing links, hard constraints → soft) and show the diff |

## 4. Safety policy
- Agents connect with role `agent`. Writes to `live` are allowed only through `commit_sandbox_to_baseline`, and only when the server policy `agent_commit = allow`. Default: `require_human`, which returns a pending-approval token for the UI.
- Every agent action is an Event with `author = Agent(name)` → full audit trail.
