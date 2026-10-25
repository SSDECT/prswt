---
type: audit
date: 2026-10-23
sources: storytold/artcraft (HEAD, shallow), storytold/gridcraft (HEAD, shallow), midasmatterhorn/artcraft-skills
---
# Audit — ArtCraft "Craft" suite conventions, and what PRSWT adopts

Scope: read-only audit of shallow clones in the session scratchpad. No code was built or run. `storytold/craftrules` (shared standards) is private and was not accessible.

## What the repos are
- **storytold/artcraft:** a Tauri + TypeScript desktop app for AI image/video generation. Little here applies to PRSWT beyond agent-doc hygiene (`AGENTS.md`, `.claude/rules/*`, file layout order, compile-time checked SQL).
- **storytold/gridcraft (and its siblings):** clean-room Rust reimplementations of commercial apps (Excel, InDesign, Premiere…). **This is the pattern to copy:** a Primavera P6-class engine is exactly the same kind of product.
- **artcraft-skills:** evidence-verified `SKILL.md` files that teach agents what each app can and cannot do. Every claim has a repo-relative pointer and a pinned commit. (Its README contains a "paste this into your agent" install prompt, which we deliberately did not execute.)

## Findings → decisions
| # | GridCraft practice | PRSWT adoption | Where |
|---|---|---|---|
| 1 | **Everything is a command**: `Session::execute(id, params)`. `CommandSpec { id, label, menu, params, enabled, run, journal, undoable }`; ids follow the target app's menus | Command registry in `prswt-app`, with ids following P6 menus/actions (`activity.add`, `relationship.add`, `schedule.run` = F9, `risk.realize`, `sandbox.create`). UI, CLI, control channel and MCP all call `execute` | [[ADR-0007-craft-conventions]] |
| 2 | **Never crash**: no `unwrap/expect/panic/todo/unimplemented/unreachable` outside tests (crate-level `deny`), `unsafe` forbidden, hostile-input rules, bounded recursion, `catch_unwind` guard in `execute`, fuzz tests over every command and random command sequences | Adopted fully. Indexing stays allowed only on engine-generated dense indices; input-derived values go through `get()` | ADR-0007, `clippy.toml` |
| 3 | **Layering enforced** by `cargo xtask layers`; the UI crate is swappable | `xtask layers` with L0 core/calendar → L1 cpm/resource/evm/risk → L2 state → L3 store/app → L4 mcp/api/ui → L5 apps | ADR-0007 |
| 4 | **One gate**: `cargo xtask ci` (fmt, clippy -D, tests, assets, layers, wasm) | `cargo xtask ci` replaces `gate.ps1` (the script becomes a thin wrapper) | ADR-0007 |
| 5 | **Parity measured, not claimed**: `catalog.rs` lists the target app's commands, and `xtask parity` generates `docs/parity-checklist.md` | A P6 feature catalog (menus, scheduling options, XER tables) generates `docs/parity-checklist.md` | ADR-0007 |
| 6 | **Progress docs**: `ROADMAP.md` (stage, headline numbers, progress log); `docs/target-app-parity.md`, `gaps.md`, `architecture.md` carry "Last reviewed/updated" headers and a revision history | Same docs in repo `docs/`; the vault keeps plan, ADRs, oracles and session logs | ADR-0007 |
| 7 | **Headless CLI** (`info, convert, eval, run --cmd, commands, mcp`) and **MCP** (headless, or `--connect` to a running app over a localhost JSON-lines control channel) | `prswt-cli`: `info, schedule, simulate, convert, run --cmd, commands, mcp [--connect]` | ADR-0006 (revised) |
| 8 | **MCP hand-written JSON-RPC** over stdio (sync, `handle_line(line) -> Option<String>`, easy to fuzz), generic `execute_command` + `list_commands` + curated tools | Adopted: hand-written sync JSON-RPC, no async runtime in the MCP path. Generic `execute_command`/`list_commands` plus the curated tools of [[04-MCP-Spec]] | ADR-0006 (revised) |
| 9 | **Rust-only UI**: egui/eframe native + WASM web; thin panels that call `app.run(id, params)`; colours from theme tokens | Replaces React/TS: `prswt-ui` (egui) + `apps/prswt` desktop + WASM target. Math crates stay wasm-safe | [[ADR-0008-rust-ui-egui]] |
| 10 | **Clean-room**: observe the target app black-box only; no vendor assets, icons or sample files; public docs only; no GPL code | Same for Oracle P6 / MS Project: feature names OK; no Oracle artwork, no copied docs text, no P6-produced fixtures in the repo | ADR-0007 |
| 11 | **Samples & fixtures built in code** (`--sample budget`); no large binary fixtures | `--sample` projects (e.g. `bridge`, `refinery-turnaround`) defined in code | ADR-0007 |
| 12 | **File formats** are a parity dimension (`file-format-parity.md`) | Add P6 **XER** import/export and MS Project **MSPDI XML** import/export to the scorecard | Scorecard P9 |
| 13 | **Undo/redo** with a bounded history | Free from event sourcing + persistent state: undo = move the head back, bounded to 100 steps | Scorecard ST-08 |
| 14 | **Agent skill**: an evidence-verified `SKILL.md` per app | Ship `skills/prswt/SKILL.md` at the end, every claim with a code pointer | Scorecard P9 |
| 15 | `clippy.toml`: unwrap/expect/panic allowed in tests; clock reads disallowed (they trap on wasm) | Adopted. The engine never reads the clock itself: time comes in via the data date / params | `clippy.toml` |
| 16 | Code style: top-to-bottom reading order (constants, primary type, supporting types, impl: ctor → public → private; tests first, helpers last) | Adopted as a rule in CLAUDE.md | CLAUDE.md |

Not adopted: GridCraft's 2-space Rust indentation (we keep rustfmt defaults); MySQL/actix conventions (not our stack).
