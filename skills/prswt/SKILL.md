---
name: prswt
description: >-
  PRSWT is a clean-room Rust project-controls and quantitative-risk engine (Primavera P6 + Primavera
  Risk Analysis class) driven through MCP, a headless CLI, an HTTP API or a desktop app. Use this skill
  whenever the user mentions PRSWT, or asks to schedule a project (CPM, critical path, float), level
  resources, compute earned value, run Monte Carlo / P50-P80 risk analysis, test mitigations in a what-if
  sandbox, or import/export a Primavera XER file. Do not assume P6 behaviour from memory; check here first.
---

# PRSWT

**Audit basis:** workspace `D:\PRSWT\engine`, 2026-10-23, gate green (118 tests). Every claim below cites a repo-relative path.

## How to drive it
1. **MCP (preferred):** `prswt-cli mcp --sample bridge`, `--db` for persisted projects, or `--connect PORT` to the desktop app started with `prswt --control PORT` (`crates/prswt-cli/src/main.rs`, `crates/prswt-mcp/src/lib.rs`). In this repo `.mcp.json` registers `prswt` and `prswt-db`.
2. **CLI:** `prswt-cli run --sample bridge --cmd 'schedule.run={}' --cmd 'simulation.run={"iterations":3000}'`; `prswt-cli import-xer FILE`, `export-xer FILE`.
3. **HTTP:** `POST /v1/commands/{id}` with JSON params, `GET /v1/events` (SSE) (`crates/prswt-api/src/lib.rs`).

All routes call the same 50-command catalog (`crates/prswt-app/src/cmds/mod.rs`); `list_commands` / `prswt://commands` gives ids and parameter docs.

## Workflow rules
- **Agents cannot write the live plan.** Do: `create_sandbox` → edits / `realize_risk` / `attach_mitigation` in it → `run_impact_simulation` with `compare_to:"live"` → `diff_branches` → ask the human to commit. `commit_sandbox_to_baseline` returns `LIVE_WRITE_FORBIDDEN` for agents (`crates/prswt-app/src/lib.rs` `guard_write`, `cmds/sandbox.rs`).
- Use the **same seed** when comparing branches: deltas are paired (common random numbers), so the differences are exact rather than noisy (`crates/prswt-risk/src/sim.rs` `uniform`, `paired_delta`).
- Durations are strings: `"5d"`, `"-2h"`, `"1.5d"`. Three-point estimates: `{"o":"5d","m":"8d","p":"15d","dist":"pert"}` (`crates/prswt-app/src/params.rs`).
- Use `explain_activity_dates` before proposing logic changes: it shows the driving chain and link slack.

## Capabilities (verified)
- **CPM:** FS/SS/FF/SF with leads/lags; lag-calendar policy; SNET/SNLT/FNET/FNLT/StartOn/FinishOn/MSO/MFO/ALAP; must-finish-by with negative float; retained logic vs progress override; LOE; milestones; WBS summaries; TF (start/finish/smallest), FF; longest path; top-K float paths; DCMA-14 (`crates/prswt-cpm/src/{passes,analysis,dcma}.rs`, oracle `vault/05-Math/CPM-oracles.md`).
- **Calendars:** inheritance, holidays, modified days, night shifts across midnight (`crates/prswt-calendar/src/lib.rs`).
- **Resources/cost/EVM:** curves, capacity, overallocation runs, serial leveling (priority, preserve float), cost rollups, PV/EV/AC, CPI/SPI, 3 EAC methods, TCPI, earned schedule (`crates/prswt-resource`, `crates/prswt-evm`).
- **Risk:** bindings to activity/WBS/resource/global; delay, duration factor, cost add/factor and resource degradation impacts; pre/post mitigation; contingent branches; secondary triggers; rank correlation; P10–P90, criticality, sensitivity, risk tornado, target probabilities (`crates/prswt-risk`, oracle `vault/05-Math/RISK-oracles.md`).
- **Mitigation proposals:** crash, fast_track, add_resource, contingency_buffer, alternative_path; deterministic and not applied until `attach_mitigation` (`crates/prswt-risk/src/mitigate.rs`).
- **Samples:** `bridge` (5×8 civil works, 14 activities, 4 risks), `turnaround` (24/7 outage, 10 activities, 3 risks) (`crates/prswt-app/src/samples.rs`).
- **XER:** PROJECT, CALENDAR, PROJWBS, TASK, TASKPRED, RSRC, TASKRSRC (`crates/prswt-formats/src/lib.rs`).
- **MS Project XML (MSPDI):** calendars with holidays, summary tasks → WBS, tasks, links with lags, resources, assignments (`crates/prswt-formats/src/mspdi.rs`); CLI `import-mspdi` / `export-mspdi`.

## Limitations (do not promise these)
From `docs/gaps.md`: MSPDI drops LOE/percent lags/summary links; XER calendars without `clndr_data` fall back to presets; no HTTP auth; no PERT network view; no roles or OBS security; leveled dates are stored as SNET constraints; undo history does not survive a reload.
