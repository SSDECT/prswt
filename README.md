# PRSWT — project controls & risk engine

A clean-room, Rust-native, MCP-first scheduling and quantitative-risk engine in the class of Oracle Primavera P6 + Primavera Risk Analysis. The math engine is headless; the desktop UI, CLI, HTTP API and MCP server are equal clients of one command catalog.

- **Scheduling:** CPM with FS/SS/FF/SF, leads/lags, lag calendars, ten constraint types, must-finish-by and negative float, retained logic / progress override, LOE, milestones, WBS rollups, multiple float paths, DCMA-14.
- **Resources & cost:** curves, capacity, overallocation, serial leveling, cost rollups. **EVM:** PV/EV/AC, CPI/SPI, three EAC methods, earned schedule.
- **Risk:** three-point estimates, risk events bound to activities/WBS/resources, pre/post mitigation, contingent branches, secondary triggers, correlation (Gaussian copula), Monte Carlo with reproducible common-random-number comparisons, risk tornado, criticality and sensitivity.
- **What-if:** O(1) sandboxes, three-way merge, undo/redo, append-only event log in PostgreSQL.
- **Agents:** MCP server with 41 tools (39 curated + 2 generic), 14 resource URIs and 5 workflow prompts; agents change live plans only through a human-approved sandbox commit.
- **Interop:** Primavera P6 XER and Microsoft Project XML (MSPDI) import/export.
- **Desktop app:** the main way to use PRSWT (local, no account; Windows, Linux, macOS). Projects are saved as `.prswt` files; every engine command has a menu entry or dialog, with dates picked from a calendar and codes picked from lists.
- **Server** (`prswt.juz.ai`): HTTP API and remote MCP for agents, plus a sign-in page that issues access tokens. No web UI ([ADR-0009](vault/04-Decisions/ADR-0009-desktop-first.md)).

## Quick start (Windows, PowerShell 7)
```powershell
cd engine
cargo build --release
.\target\release\prswt.exe                                   # desktop app
.\target\release\prswt.exe plan.prswt --view gantt            # open a file on a tab
.\target\release\prswt.exe --sample bridge --control 7979     # sample + control channel for agents
.\target\release\prswt-cli.exe run --sample bridge --cmd 'schedule.run={}' --cmd 'simulation.run={"iterations":3000}'
.\target\release\prswt-cli.exe mcp --sample bridge             # MCP over stdio
```
Claude Code picks up the project-scoped MCP servers in `.mcp.json` (`prswt`, `prswt-db`). Persistence: `docker compose --env-file .env up -d`, then add `--db` (see `.env.example`).

## Desktop packages
`tools/package-desktop.sh` builds and packages for the OS it runs on (`dist/`): a `.zip` on Windows, a `.tar.gz` with a `.desktop` entry on Linux, a `.dmg` with `PRSWT.app` on macOS. `.github/workflows/desktop-release.yml` runs it on Windows, Ubuntu and macOS (Apple Silicon and Intel) and attaches the packages to `v*` releases. Linux needs the usual desktop libraries at run time (OpenGL, xkbcommon, X11 or Wayland). Binaries are not code-signed yet, so Windows SmartScreen and macOS Gatekeeper warn on first start (macOS: right-click → Open).

## Docs
[ROADMAP](ROADMAP.md) · [architecture](docs/architecture.md) · [MCP](docs/mcp.md) · [parity checklist](docs/parity-checklist.md) · [gaps](docs/gaps.md) · design vault in `vault/` (Obsidian).

## Quality gate
`./gate.ps1` runs fmt, clippy pedantic with `-D warnings` (never-crash lints, `missing_docs` denied), nextest (with the PostgreSQL tests when `.env` exists), rustdoc `-D warnings`, doctests, cargo-deny and the layering check.
