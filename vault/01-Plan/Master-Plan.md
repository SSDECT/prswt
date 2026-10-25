---
type: plan
status: phases 0-8 delivered (2026-10-23); P9 partial
updated: 2026-10-23
---
# Master Plan — PRSWT (AI-native EPPM & Risk Engine)

Source brief: `D:\PRSWT\promt.md`. Home: [[00-Home]]. Checklist: [[Scorecard]].

## Operating rules (how I work on this project)
1. **Vault is the source of truth.** Spec, decisions, math notes, scorecard and session logs live here. Code lives in `D:\PRSWT\engine\`.
2. **Engine first.** No UI work until the CPM, Risk, Sandbox and MCP phases are green.
3. **No placeholder math.** Every algorithm ships with tests against a hand-worked oracle in `05-Math/` plus property tests (`proptest`).
4. **One scorecard item = one unit of work.** Implement → test → tick in [[Scorecard]] → log in `06-Sessions/`.
5. **Every non-trivial choice gets an ADR** in `04-Decisions/` (context, options, decision, consequences).
6. **Secrets only via Bitwarden.** Nothing secret in files, the vault, git, logs, or chat. Items live in folder `PRSWT` (e.g. `prswt/postgres`). Read at runtime: `bw get password prswt/postgres`, using the user's own unlocked `BW_SESSION`.
7. **Determinism.** Integer time (working minutes, `i64`), seeded RNG (ChaCha8, one stream per iteration), so the same input always gives bit-identical output, even when run in parallel.
8. **Gate per phase:** `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --workspace` all pass before the phase is marked done.

## Architecture in one screen (detailed in `02-Architecture/`)
Cargo workspace `engine/`:

| Crate | Responsibility | Key deps |
|---|---|---|
| `prswt-core` | IDs, domain entities, working-time units, errors | `serde`, `thiserror`, `uuid` |
| `prswt-calendar` | Work calendars, exceptions, working-minute ↔ datetime arithmetic | `chrono` |
| `prswt-cpm` | Graph build, Kahn topo sort + cycle report, forward/backward pass, 4 link types, lags, constraints, TF/FF, multi-calendar, near-critical paths | — |
| `prswt-resource` | Assignments, rate tables, usage histograms, overallocation, serial leveling | — |
| `prswt-evm` | Baselines, PV/EV/AC, CPI/SPI, EAC/ETC/VAC/TCPI | — |
| `prswt-risk` | 3-point distributions (Triangular, PERT), risk events, bindings, mitigation sub-graphs, Monte Carlo, Iman-Conover correlation, secondary triggers, P-values, criticality/tornado | `rand`, `rand_chacha`, `rand_distr`, `rayon` |
| `prswt-state` | Persistent immutable project state (`im`), command → event model, sandbox branches, diff, rebase-merge with conflict detection | `im` |
| `prswt-store` | PostgreSQL event store + snapshots, projections | `sqlx` |
| `prswt-mcp` | MCP server: hand-written JSON-RPC over stdio, control channel (ADR-0006 rev.) | — |
| `prswt-api` | REST/JSON for the web UI, same commands as MCP | `axum` |
| `prswt-ui` / `prswt-desktop` | egui desktop app (ADR-0008) | `egui`, `eframe` |
| `prswt-formats` | P6 XER import/export | — |
| `prswt-cli` | headless CLI + MCP host | — |

Why this shape: the math crates are pure (no IO), so they are fast and easy to test. MCP and the API are thin adapters over the same `prswt-state` commands. Sandboxes are O(1) forks because of structural sharing.

## Execution roadmap
| Phase | Deliverable | Exit criterion |
|---|---|---|
| **0** | Master Architectural Specification (5 sections) in `02-Architecture/` + full [[Scorecard]] + ADRs 0001–0005 | User approves spec |
| **1** | Workspace skeleton, git init, CI-style gate script, `prswt-core`, `prswt-calendar` | Calendar tests vs. oracle |
| **2** | `prswt-cpm` | Textbook networks + P6-style cases with all link types, constraints, multi-calendar match oracle |
| **3** | `prswt-resource` + `prswt-evm` | Overallocation + leveling + EVM oracle tests |
| **4** | `prswt-risk` | Analytic checks (sums of distributions, known P-values), convergence, reproducibility |
| **5** | `prswt-state` sandboxes | Fork/diff/merge/conflict property tests |
| **6** | `prswt-store` (Postgres in Docker, password from Bitwarden) | Replay(events) == in-memory state |
| **7** | `prswt-mcp` | Full spec implemented; integration test with an MCP client |
| **8** | `prswt-api` + `web/` | UI consumes the API only |

## Open items / environment
- Docker daemon not running yet; needed in phase 6 only.
- Bitwarden CLI 2026.10.0 present, vault locked. The user unlocks it and sets `BW_SESSION` in their own shell; I never see the master password.
- Obsidian app not installed; the vault is plain Markdown and opens later via "Open folder as vault" → `D:\PRSWT\vault`.
