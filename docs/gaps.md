# Gaps (ranked work list)

> **Last reviewed:** 2026-10-25

| # | Gap | Impact | Notes |
|---|---|---|---|
| 1 | MSPDI simplifications: no LOE/WBS-summary/soft start-on in MS Project; percent lags import as 0; links to summary tasks skipped; `must_finish_by` exported as FinishDate only | Lossy round trip for those features | `prswt-formats/src/mspdi.rs` |
| 3 | The local `prswt-cli api` has no authentication | Local use only (binds 127.0.0.1); the public server uses sign-in and tokens | `prswt-api`, `prswt-server` |
| 4 | Secrets in a gitignored `.env` | Temporary by user decision; move to Bitwarden | CLAUDE.md |
| 5 | No network (PERT) diagram; Gantt has no baseline bars | UI parity | `prswt-ui` |
| 6 | Out-of-sequence progress report; expected finish; suspend/resume | P6 scheduling parity | `prswt-cpm` |
| 7 | Roles; OBS-based security | P6 resource/security parity | |
| 8 | Leveling persists leveled starts as SNET constraints | Differs from P6, which stores leveled dates separately | `resource.level` |
| 9 | Undo/redo stacks are not persisted | After reload only new changes can be undone (the event log is complete) | `prswt-state` |
| 10 | Resource-degradation impacts with uncertain factors are sampled as capacity share without the 1/x transform | Rare; a warning is emitted | `prswt-risk` sim compile |
| 11 | Alternative-path mitigations leave the replaced activity as an open end | It can still count toward the finish if long | `mitigate.rs` |
| 12 | egui default fonts are bundled (OFL/Ubuntu licences) | GridCraft keeps fonts in a separate repo | `deny.toml` |
| 13 | Desktop binaries are not code-signed | SmartScreen / Gatekeeper warnings | needs Authenticode and Apple Developer ID certificates |
| 14 | Desktop: no table sorting/filtering, no column layouts, no printing or PDF export | P6 usability parity | `prswt-ui` |
| 15 | Desktop: one window; several projects are switched in the toolbar | No side-by-side compare | `prswt-ui` |

## Cloud projects (SYNC-01..04)
- A field changed on both sides keeps the server's value; the app lists the overwritten local values but cannot re-apply them with one click.
- Linux keeps the desktop token in the kernel keyring, which does not survive a reboot (sign in again). Windows and macOS use the system keychain.
- Presence counts open watch streams; a desktop that loses its network shows as present until the server notices the dropped connection.
- Other organizations' accounts need the Azure app registration switched to multi-tenant, `MICROSOFT_ALLOWED_TENANTS` and the `xms_edov` optional claim.
- A local change that no longer fits after a sync (an edit or link to something deleted on the server) is listed as a conflict with field `*` and your value; it is not kept.
- Accepting a proposal on one copy merges it there; the server checks only that the sandbox was open and the caller may commit, not that live already holds the sandbox's changes (review M9, second half).
- A project opened as a plain `.prswt` file cannot become a cloud copy by opening it from the cloud (close the file first): linking would count unsent differences as in sync.
- Viewers cannot make sandboxes on a shared server (a sandbox is a write); they run risk analysis on a downloaded copy.

## Desktop app (DSK-01..08)
- While a long command (risk analysis, leveling) runs, the workspace is locked, so the window shows a progress panel instead of the views. It keeps repainting; there is no cancel yet.
- Reports (history, float paths, EVM, health check) are shown as an expandable result tree, not formatted reports.
- Undo history starts empty after opening a `.prswt` file (gap 9).
