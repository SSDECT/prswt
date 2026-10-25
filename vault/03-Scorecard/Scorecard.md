---
type: scorecard
updated: 2026-10-25
---
# Scorecard

Legend: `[ ]` todo · `[~]` in progress · `[x]` done + tested. IDs are stable; test names carry the ID (e.g. `cpm_07_ff_link`). Spec: [[01-Stack]] [[02-Ontology]] [[03-Pipeline]] [[04-MCP-Spec]].

## P0 — Specification
- [x] SPEC-01 Stack & justification
- [x] SPEC-02 Domain ontology
- [x] SPEC-03 Math & simulation pipeline
- [x] SPEC-04 MCP specification
- [x] SPEC-05 Scorecard
- [x] ADR-0001…0006

## P1 — Foundation (`prswt-core`, `prswt-calendar`)
- [x] FND-02 Never-crash lints (deny unwrap/expect/panic outside tests), `clippy.toml`, `.env` config (Bitwarden later)
- [x] FND-03 `cargo xtask` (ci, layers, parity) replacing gate.ps1 internals
- [x] FND-01 Cargo workspace, lints, `gate.ps1` (fmt + clippy + nextest), git init, `.gitignore`, `deny.toml`
- [x] CORE-01 ID newtypes (Uuid v7), serde
- [x] CORE-02 `Instant`, `WorkMinutes`, `Dur` parsing/formatting ("5d", "-2h") with hours_per_day
- [x] CORE-03 `Money` (rust_decimal) + arithmetic
- [x] CORE-04 Entity structs per [[02-Ontology]] §2
- [x] CORE-05 Error model (codes per [[04-MCP-Spec]] §0)
- [x] CAL-01 Weekly pattern + validation
- [x] CAL-02 Exceptions (holiday, modified day) + base inheritance
- [x] CAL-03 Interval index over horizon (prefix sums), lazy extension
- [x] CAL-04 `snap_start`, `snap_finish`
- [x] CAL-05 `add_work` (±), `sub_work`, `work_between`; round-trip property tests
- [x] CAL-06 24h calendar fast path; oracle tests in [[CAL-oracles]]

## P2 — CPM (`prswt-cpm`)
- [x] CPM-01 Compile to ScheduleModel (CSR, dense ids)
- [x] CPM-02 Kahn topo sort + cycle extraction
- [x] CPM-03 Forward pass FS
- [x] CPM-04 SS, FF, SF links
- [x] CPM-05 Positive/negative lags, lag calendar policy
- [x] CPM-06 Backward pass, all link types
- [x] CPM-07 Soft constraints (SNET, SNLT, FNET, FNLT, StartOn, FinishOn)
- [x] CPM-08 Hard constraints (MSO, MFO), must_finish_by, negative float
- [x] CPM-09 ALAP
- [x] CPM-10 Total float (Start/Finish/Smallest), free float per link type
- [x] CPM-11 Multi-calendar networks
- [x] CPM-12 Milestones, LOE, WBS summary rollup
- [x] CPM-13 Progress: data date, actuals, retained logic vs progress override
- [x] CPM-14 Critical (TF threshold, Longest Path), driving links
- [x] CPM-15 Near-critical top-K path enumeration
- [x] CPM-16 `explain_dates` driver chains
- [x] CPM-17 DCMA-14 health check
- [x] CPM-18 Property tests (invariants 1–3) + benchmark 10k/50k < 10 ms

## P3 — Resources & EVM (`prswt-resource`, `prswt-evm`)
- [x] RES-01 Assignments, curves, time-phased spread on resource calendar
- [x] RES-02 Effective-dated max units & rates, 5 rate types
- [x] RES-03 Usage histograms (hour/day/week), overallocation detection
- [x] RES-04 Serial SGS leveling with priority rules, preserve-float option
- [x] RES-05 Resource-dependent duration
- [x] COST-01 Assignment cost, expenses, rollup by WBS/OBS/cost account
- [x] EVM-01 Baseline capture (state pointer) + time-phased budget
- [x] EVM-02 PV, EV (per pct type), AC
- [x] EVM-03 CPI, SPI, CV, SV, EAC (3 methods), ETC, VAC, TCPI
- [x] EVM-04 Earned schedule SPI(t); safe division (no NaN)

## P4 — Risk engine (`prswt-risk`)
- [x] RSK-01 Distributions: triangular, PERT, uniform, discrete; inverse CDFs; moment tests
- [x] RSK-02 Risk, bindings resolution (activity/resource/WBS/global)
- [x] RSK-03 Impact profiles applied to model arrays
- [x] RSK-04 Pre/post states, preventive mitigation application
- [x] RSK-05 Contingent subgraph compile + activation (active flags, replaced links)
- [x] RSK-06 Secondary trigger DAG + validation
- [x] RSK-07 Correlation: Spearman→Pearson, Higham PSD repair, Cholesky, Gaussian copula
- [x] RSK-08 ChaCha8 per-iteration streams; thread-count invariance test
- [x] RSK-09 Monte Carlo loop with scratch reuse, batching + convergence
- [x] RSK-10 Quantiles, histogram, CDF, joint confidence
- [x] RSK-11 Criticality, sensitivity (Spearman), cruciality, risk tornado
- [x] RSK-12 Common random numbers paired delta
- [x] RSK-13 Deterministic RealizeRisk pipeline ([[03-Pipeline]] §B) + delta report
- [x] RSK-14 Analytic validation (serial sum of triangulars, parallel max) + benchmark 5k×5k < 10 s
- [x] RSK-15 Mitigation subgraph generator (5 strategies)

## P5 — State & sandboxes (`prswt-state`)
- [x] ST-01 ProjectState on `im` structures
- [x] ST-02 Command enum, validation, apply → Event (with `touched`)
- [x] ST-03 Branch: fork O(1), head/version tracking
- [x] ST-04 Structural diff
- [x] ST-05 Rebase-merge + conflict detection + strategies
- [x] ST-06 TTL expiry, discard
- [x] ST-07 Property tests invariants 4–6
- [x] ST-08 Undo/redo (bounded 100 steps)
- [x] APP-01 `prswt-app` service layer, compiled-model cache per (branch, version)
- [x] APP-02 `CommandSpec` registry (P6-style ids), `execute(id, params)` with panic guard
- [x] APP-03 Fuzz: every command with empty/garbage params; random command sequences vs invariants
- [x] APP-04 Built-in sample projects (`--sample`)

## P6 — Persistence (`prswt-store`)
- [x] DB-01 docker-compose Postgres, password from gitignored `.env` (Bitwarden later)
- [x] DB-02 sqlx migrations: events, snapshots, branches, simulation_runs
- [x] DB-03 Append with optimistic concurrency
- [x] DB-04 Snapshots + replay; replay == in-memory property test
- [x] DB-05 Persist / load sandboxes and simulation runs

## P7 — MCP (`prswt-mcp`)
- [x] MCP-01 Hand-written JSON-RPC server (stdio) + control-channel Remote backend (ADR-0006 rev.)
- [x] MCP-02 Resources (§1, 14 URIs)
- [x] MCP-03 Introspection tools
- [x] MCP-04 Editing tools + `apply_commands`
- [x] MCP-05 Scheduling / resource tools
- [x] MCP-06 Baseline / EVM tools
- [x] MCP-07 Risk tools
- [x] MCP-08 Simulation tools
- [x] MCP-09 Sandbox tools + safety policy
- [x] MCP-10 Prompts (5)
- [x] MCP-11 End-to-end test with an MCP client + MCP Inspector check
- [x] MCP-12 Registered for this repo via `.mcp.json` (prswt = sample, prswt-db = persisted)

## P8 — API & UI
- [x] API-01 axum REST: GET /v1/commands, POST /v1/commands/{id}, error→HTTP status mapping
- [x] API-02 SSE /v1/events fed by the workspace event hook
- [x] UI-01 egui/eframe app shell (ADR-0008), theme tokens, control channel
- [x] UI-02 Painted Gantt (bars, links, critical highlighting, baseline bars)
- [x] UI-03 Activity table + editing through commands
- [x] UI-04 Risk register, simulation dashboards (histogram, S-curve, tornado)
- [x] UI-05 Sandbox compare & approve-merge flow
- [x] UI-06 Resource histogram, EVM panel

## P9 — Parity, formats, agents
- [x] DOC-02 Every module/public item documented (`missing_docs` denied) (from [[Audit-ArtCraft]])
- [x] CLI-01 `prswt-cli`: info, schedule, simulate, run --cmd, commands, mcp
- [x] FMT-01 P6 XER import/export (core tables: PROJECT, CALENDAR, PROJWBS, TASK, TASKPRED, RSRC, TASKRSRC)
- [x] FMT-02 MS Project MSPDI XML import/export
- [x] DOC-01 ROADMAP.md, docs/architecture.md, target-app-parity.md, gaps.md, parity-checklist (generated)
- [x] SKL-01 Evidence-verified `skills/prswt/SKILL.md`

## P10 — Server (prswt.juz.ai)
- [x] SRV-01 `prswt-server`: API + MCP + sign-in in one axum binary
- [x] SRV-02 HTTPS on the origin (rustls, hot certificate reload) + Let's Encrypt DNS-01 auto-renewal (`cert-renew` service)
- [x] AUTH-01..04 Google/Microsoft OIDC, sessions, personal access tokens, allowlist
- [x] OPS-01/02 Docker deployment on VM 130, Cloudflare tunnel route (origin certificate verified)
- [x] SRV-03 Trimmed to API + MCP + sign-in + token page; WASM web UI and `prswt-web` removed; redeployed ([[ADR-0009-desktop-first]])

## P11 — Desktop app ([[Desktop-Plan]])
- [x] DSK-01 `.prswt` project files: `file.save_project` / `file.open_project`, hostile-file checks
- [x] DSK-02 File menu: New / Open / Open recent / Save / Save as / Import / Export / Close / Quit, native dialogs, atomic saves, drag-and-drop, "save changes?" on close and quit
- [x] DSK-03 Settings: window size, theme, recent files (eframe storage)
- [x] DSK-04 Menu bar (File, Edit, View, Project, Help) + Ctrl/⌘ shortcuts
- [x] DSK-05 Long commands (risk analysis, leveling) on a background thread; the window keeps repainting
- [x] DSK-06 Dialogs for every command (calendar date pickers, code pick lists via `project.lookups`); coverage test over `commands.list`
- [x] DSK-07 Visual polish: P6-style Gantt with date axis and table, start screen, readable dates and money, app style, icon; design B (modern workspace: navigation, inspector, Ctrl+K palette, Inter, light/dark)
- [~] DSK-08 Release builds: Windows + Linux packages done; macOS via CI once the repo is on GitHub

## P12 — Cloud projects ([[Sync-Plan]], [[ADR-0010-cloud-sync-collaboration]])
- [x] SYNC-01 Members and roles, scoped engine access, desktop sign-in (loopback + PKCE), save to / open from cloud
- [x] SYNC-02 Offline-first sync: rebase with conflict reports, cached copies, background sync
- [x] SYNC-03 Proposals from people and agents; Accept / Reject / Withdraw in the app
- [x] SYNC-04 Live updates and presence (SSE), settings sync, read-only agent tokens
- [x] SYNC-05 Deployed to prswt.juz.ai (2026-10-25, Juz.ai tenant); other organizations need the multi-tenant switch in `docs/gaps.md`
- [x] SYNC-06 Review fixes ([[fix-prompt-2026-10-25-cloud-review]]): failed syncs undone as a whole, ids kept by whole-entity writes, roles on every write, read-only tokens, owner claims, SSE limits, verified Microsoft emails, desktop streams stop, file dialogs in front
