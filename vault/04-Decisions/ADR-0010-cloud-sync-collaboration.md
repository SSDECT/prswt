---
type: decision
status: accepted
date: 2026-10-25
---
# ADR-0010: Cloud projects, sync and shared work

## Context
The desktop app is the product and works locally without an account ([[ADR-0009-desktop-first]]). The user wants projects (and settings) kept on the server so they sync between computers and several people can work on one plan.

What PRSWT already has:
- Every branch is a base state plus an append-only event log; a `.prswt` file is exactly that ([[ADR-0003-persistent-state-events]]).
- The server stores events in Postgres ([[ADR-0005-postgres-event-store]]); it has Microsoft sign-in, personal tokens, the HTTP API and MCP at `prswt.juz.ai`.
- Sandboxes (what-if branches) with diff and a human-only "commit to live".
- Every change is a catalog command with an author, so history and audit are free.

## Options
1. **Files in a shared folder** (OneDrive / SharePoint / Dropbox). No server work; sync comes from the folder. One editor at a time (a lock file), no live updates, conflicting copies when two people save.
2. **Cloud projects on prswt.juz.ai, offline-first event sync** (recommended). The server holds each project's event log; the desktop keeps a local copy, works offline and exchanges events.
3. **Real-time co-editing (CRDT, Google-Docs style).** Not a fit: a schedule must be recomputed as one consistent network (CPM, leveling), and merging keystrokes does not keep it valid. Much more work for little gain over option 2.

## Proposal (option 2)
- **Local stays the default.** Signing in is needed only for cloud projects. A cloud project is a normal project with a link to the server; it can be exported to `.prswt` at any time.
- **Sync = exchange events.** The desktop sends its new events with the revision it built on. If the server is still at that revision, they are appended. If someone else wrote first, the desktop fetches their events and replays its own on top (the engine re-validates each command). A command that no longer applies, such as an edit to an activity someone deleted, becomes a visible conflict for the user to resolve. It is never silently dropped.
- **Who changes the live plan.** Editors work in their own sandboxes and propose a commit to live. Owners, and editors with permission, accept it. This is the workflow PRSWT already has for agents.
- **Sharing:** owner / editor / viewer per project, invited by Microsoft account email. Viewers see live updates and can run risk analysis in a private sandbox.
- **Live updates:** the server pushes new revisions over Server-Sent Events, with polling as a fallback. The desktop shows "Updated by Anna, 2 min ago", who is online, and a project activity feed (from the history it already has).
- **Settings sync:** per-user theme, recent cloud projects and Gantt preferences on the server. Window size and position stay local.
- **Security:** the desktop signs in with Microsoft through the system browser (OAuth device or loopback flow, PKCE). Its token is kept in the OS keychain (Windows Credential Manager, macOS Keychain, Secret Service). The server checks project roles on every command; agents keep sandbox-only rights.

## Phases
| Id | Item |
|---|---|
| SYNC-01 | Server: project store per user (events in Postgres), roles, `cloud.*` endpoints; desktop sign-in + "Save to cloud" / "Open from cloud" |
| SYNC-02 | Background sync with offline queue, rebase on conflict, conflict list in the UI |
| SYNC-03 | Sharing (invite by email, roles), commit requests from sandboxes |
| SYNC-04 | Live updates (SSE), presence, activity feed; settings sync |

## Decisions (user, 2026-10-25)
- **Who signs in:** any Microsoft work account, each working inside its own organization. Access is per project: people see only projects they are members of. Configuration: `PRSWT_ALLOWED_EMAILS` and `PRSWT_ALLOWED_DOMAINS` (`juz.ai`); anyone invited to a project may sign in too. Accounts from other organizations also need the Azure app registration switched to multi-tenant (`MICROSOFT_TENANT=organizations`).
- **Server:** one shared `prswt.juz.ai`; the Docker image stays available for a client that must host its own.
- **Offline:** offline-first from the start (the engine already records every change as commands).

## As built
- Engine: state command `SetEntity`, `rebase` (three-way, the server wins a field both sides changed and the conflict is reported), `sync_commands`, `Repo::reset_branch`, `BranchMeta.proposal`. In `prswt-app`: `Role`, `Access`, `Ctx.access`/`account` (a scoped caller's workspace holds only its projects; viewers cannot write; only owners close); commands `sync.link`, `sync.status`, `sync.outbox`, `sync.events`, `sync.receive`, `sync.apply`, `sandbox.propose`. The protocol is tested with two copies and a server in one process.
- Store: migration `0003_sharing` (`project_members`, token `kind` agent/read/desktop, `user_settings`).
- Server: per-request access from membership; `/v1/projects/{id}/members` (owners share), `/v1/projects/{id}/watch` (SSE `change` and `presence`), `/v1/me` and `/v1/me/settings`, desktop sign-in `/auth/desktop` (consent page, loopback redirect, PKCE) issuing a `desktop` token that acts as the person; MCP refuses desktop tokens; `/v1/events` filtered by access; read-only agent tokens.
- Desktop: cloud worker thread (reqwest, OS keychain via `keyring`, browser via `open`); File ▸ Cloud (sign in, open from cloud, save to cloud, share, sync now); cached copies under the app-data folder; sync 1.5 s after edits, on live changes and every minute; account and sync state in the navigation panel, people present and a Share button in the header, conflicts dialog, proposals with Accept / Reject / Withdraw on the What-if view; theme kept in the account settings.

## Review fixes (2026-10-25, SYNC-06)
- A write command that fails is undone as a whole (projects and sync state), so a failed `sync.apply` never loses local work; a local change that no longer fits the server's state becomes a reported conflict.
- `check_integrity` requires every entity to be filed under its own id; `SetEntity` cannot change the project id.
- Sandbox create/discard and `sync.link` are writes (viewers cannot); a project holds at most 1,000 branches. `sync.link` / `sync.outbox` / `sync.apply` and `project.close` are refused on a shared server. Read-only callers (`Ctx::read_only`) write nothing.
- The server makes a project's creator its owner through `claim_project`, which refuses an id that is stored or has members; a failure fails the command. The last owner cannot leave. Orphans are adopted once per database (`server_flags`).
- Watch streams are capped (8 per account, 200 per project, 5,000 in all) and end when the person is removed. Saving after a write fails only for projects that request changed.
- Microsoft sign-in needs `MICROSOFT_TENANT`; multi-tenant needs `MICROSOFT_ALLOWED_TENANTS` and a verified email (`xms_edov`).
- Desktop: streams stop on close and sign-out (and time out when silent), sign-in waits on its own thread, sign-out revokes the token, failed uploads stay local, closed projects leave no sync state, the cache is written after every sync attempt, file dialogs open in front of the window.

## Former open questions
- Who may sign in: only `juz.ai` Microsoft accounts, or also clients' Microsoft accounts (multi-tenant)?
- Must one company's data stay on its own server (self-hosting via the Docker image), or is `prswt.juz.ai` enough?
- Is offline editing with later merge needed, or is "online while editing" acceptable at first (SYNC-01 + SYNC-03 only)?
