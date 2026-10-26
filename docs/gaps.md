# Gaps (ranked work list)

> **Last reviewed:** 2026-10-26

| # | Gap | Impact | Notes |
|---|---|---|---|
| 4 | Secrets in a gitignored `.env` | Temporary by user decision; move to Bitwarden | CLAUDE.md |
| 7 | Roles; OBS-based security | P6 resource/security parity | Needs a design (role entity, staffing, access per OBS node) |
| 12 | egui default fonts are bundled (OFL/Ubuntu licences) | GridCraft keeps fonts in a separate repo | `deny.toml`; licences allowed |
| 13 | Desktop binaries are not code-signed | SmartScreen / Gatekeeper warnings | Waiting for SignPath and the Apple secrets |
| 16 | MSPDI: SS/SF from a summary task and FF/SF into one have no exact equivalent | Those links are skipped on import | `prswt-formats/src/mspdi.rs` |
| 17 | Desktop: one window; comparison is an overlay on the Gantt (another branch or project) | No two windows side by side | `prswt-ui` |

Closed on 2026-10-26: MSPDI losses (1, now FMT-03), local API without authentication (3, API-03), network diagram and baseline bars (5, DSK-10/11), out-of-sequence / expected finish / suspend-resume (6, CPM-19), leveling as SNET constraints (8, RES-06), undo history not saved (9, ST-09), degradation sampling (10, RSK-16), alternative path open end (11, RSK-16), table sorting, filtering, columns, printing (14, DSK-09).

## Cloud projects (SYNC-01..04)
- A field changed on both sides keeps the server's value; the conflicts dialog lists your values and puts them back with "Use mine" (`sync.reapply`).
- Linux keeps the desktop token in the kernel keyring, which does not survive a reboot (sign in again). Windows and macOS use the system keychain.
- Presence counts open watch streams; a desktop that loses its network shows as present until the server notices the dropped connection.
- Other organizations' accounts need the Azure app registration switched to multi-tenant, `MICROSOFT_ALLOWED_TENANTS` and the `xms_edov` optional claim.
- A local change that no longer fits after a sync (an edit or link to something deleted on the server) is listed as a conflict with field `*` and your value; "Use mine" restores it when it fits again, otherwise the integrity check refuses it.
- A project opened as a plain `.prswt` file cannot become a cloud copy by opening it from the cloud (close the file first): linking would count unsent differences as in sync.
- Viewers cannot make sandboxes on a shared server (a sandbox is a write); they run risk analysis on a downloaded copy.

## Desktop app (DSK-01..08)
- While a long command (risk analysis, leveling) runs, the workspace is locked, so the window shows a progress panel instead of the views. Risk analysis stops within one batch on Cancel (Esc); leveling cannot be stopped.
- Reports (history, float paths, EVM, health check) are shown as tables generated from the result; there are no report templates.
