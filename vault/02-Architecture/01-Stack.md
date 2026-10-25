---
type: spec
section: SPEC-01
status: draft
updated: 2026-10-23
---
# SPEC-01 — System Architecture & Stack Justification

Up: [[Master-Plan]] · Next: [[02-Ontology]]

## 1. Computational profile (what the stack must be good at)
| Workload | Shape | Target (reference HW: 8 cores) |
|---|---|---|
| CPM recalculation | O(V+E) forward + backward pass over a DAG, calendar arithmetic per edge | 10k activities / 50k links < **10 ms** |
| Monte Carlo | N iterations × full CPM, embarrassingly parallel | 5k iter × 5k activities < **10 s** |
| Sandbox fork | Copy the whole project state | **O(1)**, < 1 ms, regardless of size |
| Diff / merge | Compare two versions of state | O(changed entities) |
| MCP / API | Many small read requests, few writes | p99 < 50 ms for reads on a cached schedule |

## 2. Decisions

### 2.1 Language: Rust ([[ADR-0001-rust-core]])
- No GC pauses during long simulations. Data lives in flat arrays (`Vec<i64>`), which keeps memory access fast in hot loops.
- `rayon` gives data parallelism with no data races; the borrow checker proves the simulation workers share nothing mutable.
- Newtypes make unit errors compile errors: `WorkMinutes` ≠ `Instant` ≠ `Money`.
- One binary hosts the engine, MCP server and HTTP API: no FFI and no serialization between "engine" and "server".

### 2.2 Time model: integer minutes ([[ADR-0002-integer-time]])
- `Instant` = `i64` minutes since 2000-01-01T00:00 in project-local time. `WorkMinutes` = `i64` working duration.
- Calendars precompute sorted working intervals over the project horizon. `add_work(t, d)`, `sub_work(t, d)` and `work_between(a, b)` are binary searches over prefix sums of working minutes: O(log n), exact, no floating-point drift.
- Convention: a **start** snaps forward to the next working minute; a **finish** is the instant right after the last working minute (it snaps backward to the end of the previous working period).

### 2.3 Graph representation: compiled CSR model
- The editable state (maps keyed by UUID) is **compiled** into a `ScheduleModel`: dense `u32` indices, CSR successor/predecessor arrays, a cached topological order, and per-node calendar index, duration and constraints.
- Compiling is O(V+E). Monte Carlo compiles once and then only rewrites the duration/cost arrays each iteration, so there is no allocation in the hot loop.
- Cycle detection: Kahn's algorithm; on failure, a DFS extracts and reports the exact cycle.

### 2.4 State & branching: persistent data structures + event sourcing ([[ADR-0003-persistent-state-events]])
- `ProjectState` is built from `im::HashMap` / `im::OrdMap` (HAMT / B-tree with structural sharing). `clone()` is O(1), and an edit copies only the path to the changed node.
- Every change is a typed **Command**, validated, then recorded as an **Event**. State = fold(events). A branch is `(parent, fork_version, events since fork, head state)`.
- Sandbox = branch. Fork = clone the head pointer. Diff = structural compare that skips shared subtrees (pointer equality), so it costs O(changes).
- Merge = **rebase**: replay the sandbox's events onto the current live head. A conflict exists when the same `(entity, field)` changed on both sides since the fork. Policy: reject, ours or theirs, per field.

### 2.5 Stochastic engine ([[ADR-0004-monte-carlo-rng-copula]])
- RNG: `rand_chacha::ChaCha8Rng` seeded with `(seed)` and stream = iteration index. Results are bit-identical regardless of thread count or scheduling.
- Correlation: a Gaussian copula per correlation group. Spearman targets are converted with ρₙ = 2·sin(πρₛ/6), the matrix is repaired to PSD if needed (Higham nearest-correlation), Cholesky-factored once, and each iteration computes z → Lz → Φ → inverse CDF. This streams with no N×K pre-sampled matrix.
- Paired comparison (sandbox vs live) uses **common random numbers**: same seed, same variable ordering. The delta distribution then has much lower variance than two independent runs.

### 2.6 Persistence: PostgreSQL 17 event store ([[ADR-0005-postgres-event-store]])
- The graph is computed **in memory**. The database is for durability, audit and multi-user access, not traversal. A graph DB would add network hops to the hottest path for no gain.
- Tables: `events(stream_id, version, type, payload jsonb, author, ts)` with a unique key on `(stream_id, version)` for optimistic concurrency; `snapshots(stream_id, version, state bytea)`; `simulation_runs`.
- Access: `sqlx` with compile-time-checked queries and migrations through `sqlx-cli`. Runs locally in Docker (`postgres:17-alpine`); the password comes from Bitwarden `prswt/postgres` at runtime.

### 2.7 Interfaces: MCP + HTTP as equal adapters ([[ADR-0006-mcp-transport]])
- `rmcp` (official Rust MCP SDK): **stdio** transport for local agents (Claude Code / Desktop), **streamable HTTP** for remote agents.
- `axum` REST/JSON API for the web UI.
- Both call one `prswt-app` service layer: `execute(branch, Vec<Command>) -> Outcome` and query functions. No logic lives in the adapters.

### 2.8 Frontend (Phase 8)
React + TypeScript + Vite. Gantt and network diagram rendered on canvas (the DOM is too slow for 10k bars). It consumes only the HTTP API.

## 3. Crate map
```
engine/
  crates/
    prswt-core       ids, newtypes (Instant, WorkMinutes, Money), entities, Command/Event enums, errors
    prswt-calendar   calendars, exceptions, working-time arithmetic
    prswt-cpm        compile → ScheduleModel, topo sort, forward/backward pass, floats, paths
    prswt-resource   time-phased usage, overallocation, leveling (serial SGS)
    prswt-evm        baselines, PV/EV/AC, indices, forecasts
    prswt-risk       distributions, risk events, mitigations, copula, Monte Carlo, analytics
    prswt-state      ProjectState (im), apply(Command) → Event, branches, diff, rebase-merge
    prswt-store      Postgres event store, snapshots, replay
    prswt-app        service layer (commands + queries + caching of compiled models)
    prswt-mcp        MCP server (rmcp)
    prswt-api        HTTP API (axum)
  bins/prswt         single binary: `prswt mcp --stdio`, `prswt serve`
```
Dependency rule: math crates (`calendar`, `cpm`, `resource`, `evm`, `risk`) depend only on `prswt-core`, never on IO crates.

## 4. Quality toolchain
- `cargo nextest` (tests), `proptest` (invariants), `criterion` (benchmarks for the targets in §1), `cargo deny` (licenses/advisories), clippy `-D warnings`, rustfmt.
- Oracles: hand-worked examples in `05-Math/` with their expected numbers as the test fixtures.

## 5. Money
- Deterministic costs use `rust_decimal::Decimal`: exact, and rounding happens only at reporting.
- Monte Carlo aggregation uses `f64` internally, and reported percentiles are rounded to cents. The difference is documented and tested to stay under 0.01 per iteration total.
