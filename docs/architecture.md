# Architecture

> **Last reviewed:** 2026-10-26 · **Last updated:** 2026-10-26 · **Change:** minor (event log, API listeners, formats) · **Target:** Primavera P6

How PRSWT is built today. Rules for contributors are in [`CLAUDE.md`](../CLAUDE.md); design rationale is in the vault (`vault/02-Architecture`, `vault/04-Decisions`).

## Workspace and layering
Enforced by `cargo xtask layers`. Nothing below L3 may depend on IO, async or GUI crates.

| Layer | Crate | Job |
|---|---|---|
| L0 | `prswt-core` | Ids, `Instant`/`WorkMinutes` integer time, `Money` (Decimal), entities, error model |
| L0 | `prswt-calendar` | Calendar inheritance and exceptions; O(1)-ish working-time arithmetic (prefix sums + galloping search) |
| L1 | `prswt-cpm` | Compile to a dense CSR model; forward/backward passes; floats; critical/longest path; float paths; driver chains; DCMA-14 |
| L1 | `prswt-resource` | Spreading curves, capacity, histograms, overallocation, serial leveling, cost rollups |
| L1 | `prswt-evm` | Baselines, PV/EV/AC, indices, EAC methods, earned schedule |
| L1 | `prswt-risk` | Distributions, correlation (Higham + Cholesky + copula), Monte Carlo, realization plans, mitigation proposals |
| L2 | `prswt-state` | Persistent (`imbl`) project state, commands → events, sandboxes, three-way merge, undo/redo |
| L3 | `prswt-app` | Command catalog (`Workspace::execute`), agent policy, panic guard, schedule cache, samples |
| L3 | `prswt-store` | PostgreSQL event store (sqlx), snapshots, replay |
| L3 | `prswt-formats` | P6 XER and MS Project MSPDI import/export |
| L4 | `prswt-mcp` | Hand-written JSON-RPC MCP server; Headless/Remote backends; localhost control channel |
| L4 | `prswt-api` | axum REST + Server-Sent Events; bearer token; listens on 127.0.0.1 and ::1 |
| L4 | `prswt-ui` | egui views (Gantt with baseline and compare, activity table, network diagram, risks, simulation, sandboxes, resources/EVM) |
| L5 | `prswt-cli`, `prswt-desktop` | Binaries: `prswt-cli`, `prswt` |

## Data flow
1. **Everything is a command.** UI, CLI, HTTP, MCP and the control channel call `Workspace::execute(ctx, id, params)`. A `catch_unwind` guard restores the workspace if anything panics. The guard snapshots the workspace before every command. That snapshot is cheap because states and branch event logs are persistent `imbl` structures: a write costs 0.4 ms at 4,000 activities.
2. **Writes** become typed state commands, applied atomically to a cloned persistent state. Whole-state integrity (references and cycles) is checked, then the change is recorded as an event with field-level before/after JSON.
3. **Schedules** are compiled per (branch, version) and cached. Monte Carlo reuses the compiled model with per-iteration duration and activation overrides (`Variant`).
4. **Sandboxes** fork in O(1). Commit is a field-level three-way merge whose result must pass integrity checks.
5. **Persistence** appends new events and verifies the stored tip, so diverged writers are rejected. Load means replay.

## Agent control
- MCP: `prswt-cli mcp` (headless; `--db` persisted) or `--connect PORT` to a running desktop app (`prswt --control PORT`). See [`mcp.md`](mcp.md).
- Policy: agents may write only in sandboxes; committing to live needs a human (`AgentPolicy::RequireHuman`, the default).

## Known architectural gaps
See [`gaps.md`](gaps.md).

## Revision history
| Date | Change | Summary |
|---|---|---|
| 2026-10-23 | major | Created from the code |
| 2026-10-26 | minor | Persistent event log, API on both loopbacks with a token, MSPDI in formats, new UI views |
