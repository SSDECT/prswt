# PRSWT Roadmap

> **Last updated:** 2026-10-26 · **Stage:** alpha (engine complete, desktop app feature-complete for the catalog) · **Target:** Oracle Primavera P6 EPPM + Primavera Risk Analysis

## Headline numbers
| Measure | Value |
|---|---|
| P6 feature areas covered | 28 / 35 ([parity checklist](docs/parity-checklist.md)) |
| Engine commands | 64 (one catalog for UI, CLI, HTTP and MCP) |
| MCP | 39 curated tools + `execute_command` + `list_commands`, 14 resource URIs, 5 prompts |
| Automated tests | 200 (oracle, property, fuzz, database, headless UI) |
| CPM performance | 10k activities / 50k links < 10 ms per full recalculation |
| Monte Carlo performance | 5k activities × 5k iterations ≈ 1.5 s |
| Crates | 16, layering enforced by `cargo xtask layers` |

## Phases
| Phase | Status |
|---|---|
| 0 Specification (vault `02-Architecture`) | ✅ |
| 1 Foundation: core types, calendars | ✅ |
| 2 CPM, float paths, DCMA-14 | ✅ |
| 3 Resources, cost, leveling, EVM | ✅ |
| 4 Risk engine & Monte Carlo | ✅ |
| 5 State, sandboxes, service layer | ✅ |
| 6 PostgreSQL event store | ✅ |
| 7 MCP server, CLI, control channel | ✅ |
| 8 Desktop UI (egui), HTTP API | ✅ desktop app: files, menus, every command, background runs, packages (DSK-01..08) |
| 9 Parity, formats, agent skill | ✅ XER, MSPDI, skill |

## Next (ranked, from [gaps](docs/gaps.md))
3. Network (PERT) diagram, baseline bars in the Gantt.
4. Authentication for the HTTP API; Bitwarden-backed secrets.
5. Roles, OBS security, expected-finish / suspend-resume.

## Progress log
| Date | Change |
|---|---|
| 2026-10-25 | Cloud review fixes (SYNC-06): failed syncs undone, roles on every write, read-only tokens, verified Microsoft emails, desktop streams stop; file dialogs open in front of the window |
| 2026-10-25 | Cloud projects (ADR-0010): sharing with roles, desktop sign-in through the browser, offline-first sync with conflict reports, live updates and presence, proposals from people and agents |
| 2026-10-25 | Desktop app: `.prswt` files, P6-style menus, dialogs for every command (pickers, calendar dates), background risk analysis, redesigned Gantt, packages for Windows/Linux/macOS; server trimmed to API + MCP + tokens |
| 2026-10-24 | MS Project XML (MSPDI) import/export; server at prswt.juz.ai (sign-in, tokens, remote MCP, HTTPS with auto-renewal) |
| 2026-10-23 | Phases 0–8 built in one session; ArtCraft/GridCraft conventions adopted; XER; full docs enforcement |
