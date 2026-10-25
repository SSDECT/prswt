---
type: plan
status: active
updated: 2026-10-25
---
# Sync plan: cloud projects, sharing, live updates

Decision: [[ADR-0010-cloud-sync-collaboration]] (user, 2026-10-25).

| Id | Item | Exit criterion |
|---|---|---|
| SYNC-01 | Organizations by membership: project members and roles, scoped engine access, desktop sign-in, upload / open from cloud | two people: only members see a project; viewer cannot write; owner shares; desktop signs in through the browser |
| SYNC-02 | Offline-first sync with rebase and conflict reports | concurrent edits to different things both survive; same field: server kept, conflict reported; edits to deleted items dropped with a report |
| SYNC-03 | Proposals: sandboxes offered for review by people and agents; Accept / Reject in the app | agent's offline sandbox and proposal reach the server and other copies; agents cannot commit to live through sync |
| SYNC-04 | Live updates (SSE), presence, settings sync, read-only agent tokens | presence follows watch streams; theme follows the account |

## Status (2026-10-25)
All four built and tested (engine, store against PostgreSQL, server router, desktop end to end against a local server: download, push of an agent's sandbox + proposal, What-if review list).
Waiting: deployment to VM 130 (needs the user's go-ahead), Azure app registration multi-tenant switch for other organizations.
