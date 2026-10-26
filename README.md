# PRSWT — project controls & risk engine

A clean-room, Rust-native, MCP-first scheduling and quantitative-risk engine in the class of Oracle Primavera P6 + Primavera Risk Analysis. The math engine is headless; the desktop UI, CLI, HTTP API and MCP server are equal clients of one command catalog.

## Download

| Windows 10/11 | macOS (Apple Silicon) | macOS (Intel) | Linux x64 |
|---|---|---|---|
| [**PRSWT-windows-x64.zip**](https://github.com/SSDECT/prswt/releases/latest/download/PRSWT-windows-x64.zip) | [**PRSWT-macos-apple-silicon.dmg**](https://github.com/SSDECT/prswt/releases/latest/download/PRSWT-macos-apple-silicon.dmg) | [**PRSWT-macos-intel.dmg**](https://github.com/SSDECT/prswt/releases/latest/download/PRSWT-macos-intel.dmg) | [**PRSWT-linux-x64.tar.gz**](https://github.com/SSDECT/prswt/releases/latest/download/PRSWT-linux-x64.tar.gz) |
| Unzip, run `prswt.exe` | Open, drag to Applications | Open, drag to Applications | Unpack, run `prswt` |

No installer, no account, works offline. Start with **File ▸ Open sample**, or open the tutorial project [docs/examples/office-building.prswt](docs/examples/office-building.prswt). [User guide (RU)](docs/user-guide.md) · [all releases](https://github.com/SSDECT/prswt/releases)

## Features
- **Scheduling:** CPM with FS/SS/FF/SF, leads/lags, lag calendars, ten constraint types, must-finish-by and negative float, retained logic / progress override, LOE, milestones, WBS rollups, multiple float paths, DCMA-14, expected finish, suspend/resume, out-of-sequence report.
- **Resources & cost:** curves, capacity, overallocation, serial leveling (leveled dates kept apart from constraints, like P6), cost rollups. **EVM:** PV/EV/AC, CPI/SPI, three EAC methods, earned schedule.
- **Risk:** three-point estimates, risk events bound to activities/WBS/resources, pre/post mitigation, contingent branches, secondary triggers, correlation (Gaussian copula), Monte Carlo with reproducible common-random-number comparisons, risk tornado, criticality and sensitivity.
- **What-if:** O(1) sandboxes, three-way merge, undo/redo, append-only event log in PostgreSQL.
- **Agents:** MCP server with 41 tools (39 curated + 2 generic), 14 resource URIs and 5 workflow prompts; agents change live plans only through a human-approved sandbox commit.
- **Interop:** Primavera P6 XER (including dated `RSRCRATE` prices and assignment actuals) and Microsoft Project XML (MSPDI) import/export. PRSWT-only values travel in a `PRSWT` custom field, so a round trip loses nothing.
- **Desktop app:** the main way to use PRSWT (local, no account; Windows, Linux, macOS). Projects are saved as `.prswt` files; every engine command has a menu entry or dialog, with dates picked from a calendar and codes picked from lists. The views are:
  - Gantt with baseline bars and "Compare with" another branch or project;
  - an activity table with sorting, filters, column choice, CSV export and printing;
  - a network (PERT) diagram.

  Long risk analyses can be cancelled.
- **Local HTTP API:** `prswt-cli api --control PORT` on 127.0.0.1 and ::1. It needs a bearer token (`PRSWT_API_TOKEN`, or a printed one).
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
`tools/package-desktop.sh` builds and packages for the OS it runs on (`dist/`): a `.zip` on Windows, a `.tar.gz` with a `.desktop` entry on Linux, a `.dmg` with `PRSWT.app` on macOS. `.github/workflows/desktop-release.yml` runs it on Windows, Ubuntu and macOS (Apple Silicon and Intel) and attaches the packages to `v*` releases. Linux needs the usual desktop libraries at run time (OpenGL, xkbcommon, X11 or Wayland). Release builds are code-signed when the signing secrets are set ([docs/signing.md](docs/signing.md)); until then Windows SmartScreen and macOS Gatekeeper warn on first start (macOS: right-click → Open).

## Docs
[User guide (RU)](docs/user-guide.md) · [ROADMAP](ROADMAP.md) · [architecture](docs/architecture.md) · [MCP](docs/mcp.md) · [parity checklist](docs/parity-checklist.md) · [gaps](docs/gaps.md) · [scenario tests](docs/scenario-tests.md) · [system test](docs/system-test-report.md) · [real-data run](docs/real-data-test-report.md) · design vault in `vault/` (Obsidian).

## Quality gate
`./gate.ps1` runs fmt, clippy pedantic with `-D warnings` (never-crash lints, `missing_docs` denied), nextest (with the PostgreSQL tests when `.env` exists), rustdoc `-D warnings`, doctests, cargo-deny and the layering check.

End-to-end checks on the release binaries are separate from the gate:
- `python tools/system_test.py` covers every interface: CLI, MCP, HTTP, DB and files.
- `python tools/scenario_test.py` runs 13 hand-worked project scenarios, from link types to 1,500-activity random networks.

## How it is built
PRSWT is written with [Claude Code](https://claude.com/claude-code) (Anthropic) under the direction of its author, who sets the goals and requirements, reviews the results and tests the app. Commits made this way carry a `Co-Authored-By: Claude` line, and the original brief is in [promt.md](promt.md). Every change passes the same gate as hand-written code: formatting, clippy, the test suite with hand-worked oracles in `vault/05-Math/`, rustdoc and license checks.

## Code signing policy
Free code signing provided by [SignPath.io](https://about.signpath.io/), certificate by [SignPath Foundation](https://signpath.org/) (Windows; application pending). macOS packages are signed with the author's Apple Developer ID and notarized. Only packages built by the `desktop-release` GitHub workflow from this repository are signed, and every release is approved by hand before signing.

- Committers and reviewers: [SSDECT](https://github.com/SSDECT) (project owner), with Claude Code as a co-author under the owner's review
- Approvers: [SSDECT](https://github.com/SSDECT)

Privacy: the desktop app sends nothing to other networked systems unless the user asks for it. The only network features are the optional cloud sync and sharing on `prswt.juz.ai`, used after **File ▸ Cloud ▸ Sign in…**.

## License
Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in PRSWT by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
