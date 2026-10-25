---
type: plan
status: active
updated: 2026-10-25
---
# Desktop plan: finish the PRSWT desktop app (Windows, Linux, macOS)

Decision: [[ADR-0009-desktop-first]] (user, 2026-10-24). The desktop app is local and needs no sign-in; the server keeps only the API, MCP and the token page.

## Work items (Scorecard section P11)
| Id | Item | Exit criterion |
|---|---|---|
| DSK-01 | Project files: `file.save_project` / `file.open_project` (versioned `.prswt` JSON: every branch's base state + events, replayed on open) | round-trip test (save → open gives the same heads, branches and history); hostile files (bad JSON, wrong version, oversized, broken events) give errors, never a crash |
| DSK-02 | File menu in the desktop shell: New, Open, Save, Save As, Import/Export XER and MSPDI through native dialogs; recent files; window title shows file + unsaved mark; ask before closing or opening with unsaved changes | atomic save (temp file + rename); tests for the shell's file logic |
| DSK-03 | Per-OS settings (config dir): window size, recent files, theme | survives restart; corrupt settings fall back to defaults |
| DSK-04 | Menu bar after P6's menus (File, Edit, View, Project, Activities, Tools, Help) + shortcuts (Ctrl/⌘ + N/O/S/Z/Y) | every user-facing command reachable from a menu |
| DSK-05 | Long commands (simulation, leveling) off the UI thread with a progress/busy state | UI keeps repainting during a 10 000-iteration run |
| DSK-06 | Editing coverage: WBS, relationships, calendars and exceptions, resources and assignments, expenses, baselines, project settings | each `CommandSpec` with a UI meaning has a UI path (checked by a test over `commands.list`) |
| DSK-07 | Visual polish: theme tokens (light/dark), spacing, typography, status bar, empty states | screenshots reviewed by the user |
| DSK-08 | Release builds: CI matrix Windows / Linux / macOS (arm64 + x86_64), artifacts zip / tar.gz / .app in .dmg; app icon | builds green on all three; Windows and Linux binaries smoke-tested here (Linux via Docker) |
| SRV-03 | Server trimmed to API + MCP + sign-in + token page; WASM web UI and `prswt-web` removed; redeployed | `/` shows the account/token page, `/v1/*` and `/mcp` unchanged |

## Status (2026-10-25)
DSK-01..07 and SRV-03 done. DSK-08: packaging script and CI workflow ready; Windows package built and smoke-tested here; Linux built in Docker on VM 130 (links only libc; GUI libraries loaded at run time; exits cleanly without a display), packaged as dist/prswt-0.1.0-linux-x86_64.tar.gz; macOS needs the CI run (repository not on GitHub yet).
User feedback that shaped DSK-06/07: no typed dates or codes — pick from calendars and lists.

## Order
DSK-01 → DSK-02 → DSK-03 → DSK-04 → SRV-03 → DSK-05 → DSK-06 → DSK-07 → DSK-08.

## Blockers / needs the user
- macOS builds need a macOS machine or a CI runner (GitHub Actions). The repo has no commits and no remote yet: the user decides where it lives.
- Code signing (Windows Authenticode, Apple Developer ID + notarisation) needs certificates the user would have to buy; until then the OS shows "unknown publisher" warnings.
