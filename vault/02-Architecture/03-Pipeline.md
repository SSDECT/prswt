---
type: spec
section: SPEC-03
status: draft
updated: 2026-10-23
---
# SPEC-03 — Mathematical & Simulation Pipeline

Prev: [[02-Ontology]] · Next: [[04-MCP-Spec]]

## A. Deterministic recalculation (`recalculate`)
Triggered after any successful command batch. The result is cached per `(branch, version)`.

1. **Validate.** Referential integrity, calendar validity, constraint compatibility, link rules. Cycle check with Kahn; on failure a DFS returns the cycle `[A → B → C → A]` as a structured error.
2. **Compile** → `ScheduleModel`: dense indices, CSR `succ[]`/`pred[]`, `topo[]`, per-node `{cal, dur, rem, cons, actuals, active}`. Contingent subgraph nodes are compiled with `active=false`.
3. **Progress normalisation** (data date `DD`):
   - completed: ES=AS, EF=AF (fixed, not recalculated)
   - in progress: ES=AS, EF = add_work(cal, max(DD, AS), remaining)
   - not started: earliest start ≥ DD
   - *Retained Logic*: the remaining work of out-of-sequence activities still waits for predecessors. *Progress Override*: it ignores them.
4. **Forward pass** (topo order). For a node j the earliest start is the max of these candidates:
   | source | candidate for ES_j |
   |---|---|
   | data date | DD (not started) |
   | FS i→j, lag L | add_work(Lcal, EF_i, L) |
   | SS i→j | add_work(Lcal, ES_i, L) |
   | FF i→j | sub_work(cal_j, add_work(Lcal, EF_i, L), dur_j) |
   | SF i→j | sub_work(cal_j, add_work(Lcal, ES_i, L), dur_j) |
   | SNET / StartOn | date (soft: max) |
   | FNET / FinishOn | sub_work(cal_j, date, dur_j) |
   | MSO / MFO | **override** ES to the date (hard) |

   Then `ES_j = snap_start(cal_j, ES_j)` and `EF_j = add_work(cal_j, ES_j, dur_j)`. Negative lag means `add_work` with a negative value (walking backward over working time).
5. **Backward pass** (reverse topo). Project late finish `LF_end = must_finish_by ?? max(EF)`. LF_i = min of:
   | source | candidate for LF_i |
   |---|---|
   | no successors | LF_end |
   | FS i→j | sub_work(Lcal, LS_j, L) |
   | SS i→j | add_work(cal_i, sub_work(Lcal, LS_j, L), dur_i) |
   | FF i→j | sub_work(Lcal, LF_j, L) |
   | SF i→j | add_work(cal_i, sub_work(Lcal, LF_j, L), dur_i) |
   | SNLT / StartOn | add_work(cal_i, date, dur_i) |
   | FNLT / FinishOn | date |
   | MSO / MFO | override |

   `LS_i = sub_work(cal_i, LF_i, dur_i)`. ALAP activities are then moved to their late dates (only within their free float).
6. **Floats** (working minutes, on the activity's calendar):
   - `TF = work_between(cal, ES, LS)` (Start mode), `work_between(EF, LF)` (Finish mode), or the min of both (Smallest, the default). Negative values are allowed.
   - `FF_i = min over successors of the slack of the link` (e.g. FS: `work_between(Lcal, add_work(EF_i, L), ES_j)`); for an end activity, `work_between(EF_i, LF_end)`.
7. **Critical & paths.**
   - Critical = `TF ≤ threshold`, or Longest Path = the activities reached by tracing **driving** links (slack 0) backward from the latest finisher.
   - Near-critical: `threshold < TF ≤ near_threshold`.
   - Path enumeration: a "driving path" DAG walk ranked by path float, returning the top-K disjoint chains (K configurable). This is used by the `critical-path` and `near-critical` resources.
8. **Resources.** Spread each assignment's units over working minutes of the assignment curve on the resource calendar → per-resource histogram per period (hour/day/week). Overallocation = periods where usage > max_units(t). Leveling = **serial schedule generation**: activities in priority order (default: TF asc, ES asc, code), each placed at the earliest time that is both logic-feasible and resource-feasible. Option `preserve_float`: shift only within TF.
9. **Cost.** Assignment cost = Σ units(t) × rate(t, rate_type); plus expenses; rolled up by WBS, OBS and cost account.
10. **EVM** (as of `DD`, against baseline B):
    - BAC = Σ baseline budget; PV = baseline budget time-phased up to DD; EV = Σ budget_i × pct_i; AC = Σ actual cost.
    - CPI = EV/AC, SPI = EV/PV, CV = EV−AC, SV = EV−PV.
    - EAC (selectable): `AC + (BAC−EV)` · `BAC/CPI` · `AC + (BAC−EV)/(CPI·SPI)`. ETC = EAC − AC, VAC = BAC − EAC, TCPI = (BAC−EV)/(BAC−AC).
    - Earned schedule: ES_t = t where PV(t) = EV (interpolated), SPI(t) = ES_t / AT.
    - Division by zero ⇒ `None` (never NaN or ∞ in outputs).

## B. Risk transition: Potential → Realized (deterministic)
Command `RealizeRisk { risk, at: Instant, impact: Realization }`. `Realization` = concrete values chosen by the caller, or "use most-likely".

1. **Guard.** The risk is Potential or Active and `at` lies inside its proximity window; else error `RISK_NOT_REALIZABLE`.
2. **Resolve targets.** Binding → set of activities/resources (WBS branch expands to its descendants; Global → project finish milestone).
3. **Apply impacts** as ordinary graph edits recorded as events (so they are diffable and revertible):
   - ScheduleDelay Δ → `remaining += Δ` (`EachActivity`) or on one target (`Once`)
   - DurationFactor f → `remaining = round(remaining × f)`
   - CostAdd / CostFactor → an expense line `risk:<id>` on the target
   - ResourceDegradation → a `max_units` override on the window
4. **Contingent mitigations** of that risk with status Accepted → **splice**: activate the subgraph nodes and anchors, and deactivate the links the anchors `replace`. Then re-check for cycles.
5. **Secondary triggers.** Each target risk becomes Active, with its probability raised to `max(p, conditional_probability)`, plus a review flag. Nothing is auto-realized deterministically.
6. **Recalculate** (Section A) and **delta report** against the previous version: Δ project finish, Δ cost, activities whose TF dropped (float consumed), activities that newly became critical or near-critical, changed driving path, new overallocations, EVM forecast delta.
7. Risk status → Realized. The event records the before/after summary.

## C. Stochastic simulation in a sandbox (`run_impact_simulation`)
1. **Fork** a sandbox from live (O(1)). The agent applies commands (inject risks, change estimates, accept mitigations).
2. **Compile a SimModel** once: the ScheduleModel plus a vector of uncertain variables `V = {duration_i, cost_k, occurs_r, impact_r,m}`, correlation groups with their Cholesky factors, the risk trigger DAG in topological order, and dormant subgraphs. `mitigation_mode` ∈ {Pre, Post, Both}; Both runs two passes with common random numbers.
3. **Per iteration n** (parallel `rayon` chunks; RNG = ChaCha8(seed, stream=n)):
   1. Draw independent uniforms in a fixed variable order. For each correlation group: z = Φ⁻¹(u), x = L·z, u' = Φ(x).
   2. Durations by inverse CDF: triangular in closed form; PERT = Beta(α, β) with α = 1+λ(m−o)/(p−o), β = 1+λ(p−m)/(p−o), using a regularized-incomplete-beta inverse; uniform; discrete.
   3. Risk occurrence (in trigger-DAG order): occurs_r = u_r < p_r(mode, proximity), where p_r is raised to conditional_probability if a parent occurred in this iteration.
   4. Apply sampled impacts to the per-iteration duration/cost arrays. Splice the contingent subgraphs of occurred risks by toggling node/edge `active` flags. Inactive links are skipped, and replaced links are dropped.
   5. Forward + backward pass on the scratch arrays (reused per worker thread: no allocation).
   6. Record: project finish, total cost, critical bitset, finish time of each tracked milestone, and the sampled inputs that analytics need.
4. **Convergence.** Run in batches of 500. Stop when |ΔP80| and |ΔP50| between consecutive batches are below ε (default 0.5 working day and 0.1 % of cost) for 2 batches in a row, or at max_iterations.
5. **Aggregate.**
   - Exact empirical quantiles P10…P90 (sorted), histogram (Freedman–Diaconis bins), CDF, mean and σ for finish and cost.
   - Joint confidence = P(finish ≤ D ∧ cost ≤ C) on a grid.
   - Criticality index CI_i = share of iterations where i is critical.
   - Duration sensitivity = Spearman ρ(duration_i, finish). Cruciality = CI_i × ρ_i.
   - Risk tornado = E[finish | occurs_r] − E[finish | ¬occurs_r], and the same for cost.
   - "Schedule sensitivity index" = CI × σ_i/σ_project.
6. **Delta vs live.** The same seed on the live branch (common random numbers) gives paired differences per iteration, so the Δ distribution gets P50/P80/P90 too: "the mitigation saves 12 days at P80 for 38 k".
7. **Store** a SimulationRun with its config hash. Identical inputs + seed ⇒ cached result.

## D. Sandbox lifecycle
`create_sandbox` → commands → `recalculate`/`simulate` → `diff_branches` → `commit_sandbox_to_baseline` **or** `discard_sandbox`.
Commit = rebase-merge (see [[01-Stack]] §2.4): conflict detection on `(entity, field)` touched on both sides since the fork. Then validate, recalculate, and optionally `capture_baseline`. Expired TTL ⇒ auto-discard.
