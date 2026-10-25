# Task for an agent: fix the review findings in cloud projects (SYNC-01..04)

> Date: 2026-10-25. Source: three read-only reviews (engine/sync, server/store, desktop/UI) of the ADR-0010 code.
> Code was not changed by the review. This document is the work order.

## Rules (from `CLAUDE.md`, mandatory)
- Read first: `CLAUDE.md`, `vault/04-Decisions/ADR-0010-cloud-sync-collaboration.md`, `vault/01-Plan/Sync-Plan.md`.
- Never crash: no `unwrap/expect/panic` outside tests; hostile input is bounded; `Result<_, EngineError>`.
- **Every fix ships with a regression test** that fails without the fix. Engine tests go in `prswt-app/src/tests.rs` (`mod cloud`), server tests in `prswt-server/src/tests.rs` (they need `PRSWT_TEST_DATABASE_URL`; the gate sets it from `.env`), store tests in `prswt-store/src/tests.rs`.
- `///` docs on every new public item; `//!` stays accurate.
- Finish with `./gate.ps1` → `GATE PASSED`. Then add an entry to `vault/06-Sessions/2026-10-23.md` and tick SYNC-06 in `vault/03-Scorecard/Scorecard.md`.
- Commit only if the user asks. Do not deploy to the server without the user's go-ahead.
- Work in order: critical → high → medium → low. When a fix changes a protocol (`sync.*`, HTTP), update the tests on both sides.

---

## CRITICAL

### C1. `sync.apply` loses local work on any error after `reset_branch`
**Where:** `prswt-app/src/cmds/sync.rs` `apply_branch` (~319-335), `prswt-app/src/lib.rs` `run_guarded` (~280-302).
**Problem:** the branch is rewound (`reset_branch` drops local events and undo) *before* it is known that the server events load and the rebase succeeds. `run_guarded` restores `repos` only on a panic, not on `Err`. Any later error (gap in versions, event under the wrong branch, an integrity failure in the rebase `execute`) leaves the branch rewound, the local work exists only in the local variable `local`, and `synced` is not updated, so the next sync "doesn't see" the local work.
**Example:** offline you added a link or assignment to activity X; someone deleted X on the server → the integrity check fails → the whole local change set is lost.
**Fix:**
1. Make `sync.apply` transactional per project: save `w.repos.get(&pid).cloned()` and the `SyncLink` before the loop; on any `Err`, put them back and clear `w.cache`. (Or, more generally, in `run_guarded` restore `saved` on `Err` too for `spec.writes` commands.)
2. In `apply_branch`, validate the events **before** `reset_branch`: all `e.branch == id`, versions contiguous from `synced+1`.
3. If the rebase `execute` fails integrity, don't fail: add the problem entities to the conflicts (`field: "*"`, `yours: <local value>`), finish with the server state and move on.
**Test:** "offline link to an activity deleted on the server": no Err, a conflict is reported, the state equals the server's, other local changes are not lost.

### C2. `SetEntity` bypasses invariants (id substitution, project id change)
**Where:** `prswt-state/src/command.rs` (`SetEntity`, ~399), `prswt-state/src/state.rs` `set_json` / `check_integrity`.
**Problem:**
- a) `set_json` doesn't check that `value.id == entity.id` → an activity with `.id = B` is stored under key A; deletes, cascades and upserts break. Reachable by any Editor through `commands.apply` and by any desktop client.
- b) For `entity.kind == Project` the `project.id` can be changed → the `.prswt` stops opening ("an event moved the project to another id"), and `sync::create_branch` rejects every offline sandbox.
**Fix (in `Command::apply` for `SetEntity`, so sync/undo paths that call `set_json` directly are unaffected):**
```rust
Self::SetEntity { entity, value } => {
    if entity.kind == EntityKind::Project {
        let v = value.as_ref().ok_or_else(|| EngineError::validation("project cannot be removed"))?;
        if v.get("id").and_then(Value::as_str) != Some(&s.project.id.to_string()) {
            return Err(EngineError::validation("project id cannot be changed"));
        }
    } else if let Some(v) = value
        && v.get("id").and_then(Value::as_str) != Some(entity.id.as_str())
    {
        return Err(EngineError::validation("entity id does not match its address"));
    }
    s.set_json(entity, value.clone())?;
    vec![entity.clone()]
}
```
Also add the invariant "map key == value.id" to `check_integrity` for every collection.
**Tests:** `SetEntity` with a different id → Validation; with a changed project id → Validation; the state is unchanged.

---

## HIGH

### H1. `sandbox.create`, `sandbox.discard`, `sync.link` are marked `writes: false`, so a viewer and a `read` token can change things
**Where:** `prswt-app/src/cmds/mod.rs` (~32, ~497-536), `authorize` in `prswt-app/src/lib.rs` (~311); the special case `starts_with("sandbox.")` in `prswt-api/src/lib.rs` (~146) and `prswt-mcp/src/lib.rs` (~110).
**Example:** a viewer discards someone else's proposed sandbox; or creates 1001 sandboxes, after which the `.prswt` stops opening (`MAX_FILE_BRANCHES`).
**Fix:** set `writes: true` on these three commands; remove the special cases in api/mcp (they become redundant); in `Repo::fork`, refuse when `branches.len() >= MAX_FILE_BRANCHES` (`ErrorCode::Limit`).
**Tests:** a viewer can't `sandbox.discard` / `sandbox.create` (Forbidden) at engine level and through the server (403); the fork limit holds.

### H2. `sync.apply` / `sync.link` / `sync.outbox` are callable on the server, so an agent can write live without the policy or integrity check
**Where:** `prswt-app/src/cmds/sync.rs` (~212-245, ~284-357).
**Problem:** `apply_loaded_event` trusts `changes` (no `check_integrity`, no `guard_write`); in the "new branch" path it accepts `kind: Live` (a second live → the file won't open), someone else's `base.project.id`, `base` without an integrity check, and unbounded name/note.
**Fix:** refuse `sync.link`, `sync.outbox`, `sync.apply` when `ctx.access.is_some()` (these are client-only commands; the server's are `sync.events`/`sync.receive`). Also in `apply_branch` for a new branch: `kind == Sandbox`, `parent` present, `base.project.id == pid`, `base.check_integrity()`; after `load_events`, `check_integrity` of the head; trim name to 80 and note to `MAX_NOTE`.
**Test:** a scoped caller gets Forbidden on `sync.apply`; a new branch with `kind: live` → Validation.

### H3. Desktop: an offline "Save to cloud" turns a local file into a "cloud" one, and Ctrl+S stops saving
**Where:** `prswt-desktop/src/cloud.rs` `run` (~154), `prswt-desktop/src/main.rs` `cloud_events` (~458-463), `is_dirty` (~579), `FileRequest::Save` (~182).
**Problem:** an offline failure of `Upload`/`Open` is reported as `Done::Offline(id)`; the handler does `c.sync.entry(id).or_default()` → the project counts as cloud → `is_dirty` = false, Ctrl+S sends a Sync (no-op), and closing doesn't ask to save. Changes are lost.
**Fix:** treat `offline` as `Done::Offline` only for `Job::Sync`; for Upload/Open send `Done::Error`. In `cloud_events`, don't create entries in `Offline`/`Changed`/`Present` (use `get_mut`, ignore unknown ids); create them only in `Opened`/`Synced`.

### H4. Desktop: after closing a cloud project its trace stays, and the same file opens as "cloud"
**Where:** `main.rs` `cloud_schedule` (~524-528), `close()`; `prswt-ui/src/cloud.rs` `selected_is_cloud` (~112).
**Fix:** on close / `rev.is_none()`, also `self.app.cloud.sync.remove(&id)` and send a new `Job::Unwatch(id)` (see H5).

### H5. Desktop: `watch_loop` threads never stop
**Where:** `prswt-desktop/src/cloud.rs` (~449-466, ~656-723).
**Problem:** on close, sign-out or server change the thread keeps going: it uses the token after sign-out, keeps the user "present" for others, sends `Job::Sync` for closed projects (error spam, ghost entries in `cloud.sync`); `watching` keeps the id, so a reopen doesn't start a new watcher; the client has no timeout (a hung TCP connection is never noticed).
**Fix:** `watching: HashMap<String, Arc<AtomicBool>>`; check the flag between lines and after each reconnect sleep; `.timeout(Duration::from_secs(60))` on the client (the server sends a keep-alive every 15 s); on `SignOut`, set every flag and clear the map; add `Job::Unwatch(id)`; when the loop exits on 401/404, remove the id from `watching` (`Done::Unwatched(id)` or a shared map).

### H6. Server: a "read only" token isn't read-only (and can widen the sign-in list)
**Where:** `prswt-server/src/main.rs` (~547-552), `cloud.rs` `set_member`/`remove_member` (~199-203); `CREATES` in `prswt-app` bypass `authorize`.
**Example:** a leaked read token runs `sample.load` → becomes Owner of the new project → invites any email → those emails can now sign in to the server.
**Fix:** in `require_caller`/`run_command`, refuse write commands (including `CREATES` and `sandbox.*`) for `caller.kind == "read"`; in `set_member`/`remove_member`, require `caller.kind != "read"`. Fix the comment in `access_of` (~508): read tokens map only the owner's memberships to viewer.
**Test:** a read token gets 403 on `sample.load` and on PUT members.

---

## MEDIUM

### M1. Server: taking over a closed project by its id
**Where:** `prswt-app/src/lib.rs` (~256-266), `cmds/project.rs` `close`, `prswt-server/src/main.rs` `on_create` (~290-299), `prswt-store/src/sharing.rs` `set_member` (`ON CONFLICT DO UPDATE`).
**Example:** an owner runs `project.close` on the server (the project leaves memory but stays in the DB); an ex-member uploads a `.prswt` with the same id → `on_create` makes them Owner of the real project.
**Fix:** (a) refuse `project.close` when `ctx.access.is_some()`; (b) make `on_create` fallible: `INSERT … ON CONFLICT DO NOTHING`, and fail the command if the project already has members or a `projects` row (change the hook signature to return `Result`).

### M2. Server: a project with no members gets owners on every restart
**Where:** `main.rs` `on_create` (only logs the error), `sharing.rs` `adopt_orphans` (~155-183), `ensure_an_owner` (~240).
**Problem:** if `set_member` fails, the project is saved with no members and on the next start everyone in `PRSWT_ALLOWED_EMAILS` becomes its Owner. The same happens when the last owner removes themself (`n == 0` is not checked).
**Fix:** a failing `on_create` fails the command (see M1); `ensure_an_owner` refuses to remove the last owner (including `n == 0` for an existing project); make `adopt_orphans` a one-time migration (only projects created before 0003, or a flag in a table) instead of a sweep on every start.
**Test:** the last owner can't remove themself; a restart doesn't add owners to a new project.

### M3. Server: no limit on SSE connections
**Where:** `prswt-server/src/cloud.rs` `Presence::join`/`watch` (~78-92, ~243-280), `prswt-api` `/v1/events`.
**Fix:** `join` returns `None` → 429 when the project already has `MAX_PRESENT` connections or one email has more than 8; add a global SSE counter.

### M4. Server: an error syncing one project breaks every write
**Where:** `main.rs` `after_write` (~302-307), `prswt-api/src/lib.rs` (~148-150), `prswt-store/src/lib.rs` `sync` (~281-315).
**Problem:** `after_write` syncs **all** projects; a `VersionConflict` in one makes every user's writes return an error (even though the change in memory was already applied).
**Fix:** collect changed project ids via `on_event` and sync only those; report other projects' errors in the log, not as the request's reply.

### M5. Server: Microsoft "nOAuth" when `tenant=common`
**Where:** `prswt-server/src/oidc.rs` `validate_claims` (~234-249), default `common` in `main.rs` (~95).
**Problem:** with `common`, the `email` / `preferred_username` claims can be faked from your own Entra tenant → sign in as `someone@juz.ai`. Production uses the tenant id (safe), but the default is dangerous and matters for the planned multi-tenant switch.
**Fix:** for Microsoft, require a concrete `MICROSOFT_TENANT`, or check `tid` against an allowlist (`MICROSOFT_ALLOWED_TENANTS`); accept `email` only with `xms_edov == true` (or use a verified identifier); drop the `preferred_username` fallback. Update `deploy/README.md` (the multi-tenant section).

### M6. Engine: the role check fails open
**Where:** `prswt-app/src/lib.rs` `authorize` (~314-316).
**Fix:** for write commands outside `CREATES`, return the project-resolution error instead of `Ok(())`.

### M7. Engine: stale schedule cache and undo stack after sync
**Where:** `sync.rs` (~237, ~323), `lib.rs` `schedule` cache `(branch, version)`; `branch.rs` `apply_loaded_event` (~524-539).
**Fix:** clear `w.cache` (and the project's `sims`) **before** `reset_branch`; when server events are appended in the no-local-changes path, clear `undo`/`redo` (as `reset_branch` does).
**Test:** after a sync without local changes, `edit.undo` doesn't overwrite someone else's change.

### M8. Engine: a failed `sample.load` leaves an orphan project
**Where:** `lib.rs` `execute` (~268-275), `project.rs` `sample` (~260-279).
**Fix:** on `Err`, remove the `created` ids from `repos`.

### M9. Engine: `adopt_state` trusts the client (forged proposal author, reopening a merged branch)
**Where:** `sync.rs` `adopt_state` (~450-478).
**Fix:** when the proposal changes, set `by = ctx.author`, `at = w.now()` on the server; allow only `Open → Discarded | Merged` and proposal changes; refuse reopening; for `Merged`, require that live already contains the sandbox head (`diff(parent, sandbox)` empty) or do the merge on the server.

### M10. Engine: `SyncLink` from a file is trusted
**Where:** `project.rs` `open_project` keep_sync (~423-429).
**Fix:** on open, drop ids missing from the repo, `synced[b] = min(synced[b], version)`, check `server` as in `sync.link`.

### M11. Desktop: sign-in through the loopback can hang or be broken by a local process
**Where:** `prswt-desktop/src/cloud.rs` `wait_for_code` (~609-651), `sign_in` (~237-256).
**Fix:** `stream.set_read_timeout(Some(5 s))`; answer non-matching requests with 400 and keep listening; return only after a successful `POST /auth/desktop/token`. Run sign-in on a separate thread so syncs don't wait 5 minutes behind it.

### M12. Desktop: `open()` when the project is already open as a plain file, or the cache is unreadable
**Where:** `cloud.rs` `open` (~338-362).
**Fix:** if `project.list` has the id but it isn't linked, run `sync.link` (the local file becomes the cloud copy). Decide whether to link from `reply["linked"]` of `file.open_project`, not from `cached.exists()`. If `sync.server` in the cache differs from `self.server`, relink.

### M13. Desktop: edits made during a sync go out only after 60 s
**Where:** `main.rs` (~439-456, ~529-541).
**Fix:** the worker returns in `Done::Synced` the revision it saw after its last write; if `app.revision(id)` differs, set `due = now + SYNC_DELAY_SECS`.

### M14. Desktop: race writing the cache file
**Where:** `cloud.rs` `write_cache`, `main.rs` `cache_cloud` (~557-574), `files.rs` `write_atomic` (one fixed temp name).
**Fix:** a unique temp name (`.{name}.{pid}.{counter}.tmp`), or route all cache writes through the worker (`Job::Cache(id)`), leaving only `save()` on exit on the main thread.

### M15. Desktop: the cache isn't written on errors
**Where:** `cloud.rs` `sync` (~369-409).
**Fix:** `let r = self.sync_inner(id); let _ = self.write_cache(id); r` (except "not linked").

### M16. Desktop: misleading errors on upload
**Where:** `cloud.rs` `upload` (~313-336).
**Fix:** don't use `contains("already")` (it also catches "a project named X is already open" from someone else's project); give the server a dedicated error code / exact message and show it; "a project with this id already exists on the server: ask its owner to share it". If the server accepted the upload but the local `sync.link` failed, go down the link path instead of failing.

### M17. Desktop: `revision()` several times per frame blocks the UI
**Where:** `main.rs` (~134-137, ~520, ~672), `prswt-ui/src/lib.rs` `revision` (~473-483).
**Fix:** one `project.list` per frame → `HashMap<String, u64>` on `Window`; `cloud_schedule` checks at most every 250 ms; on the UI path use `try_lock` (if busy, show "Syncing…").

### M18. Desktop: sign-out doesn't revoke the token on the server; `PRSWT_TOKEN` cancels sign-out
**Where:** `cloud.rs` `SignOut` (~172-180), `resume` (~279).
**Fix:** before deleting from the keychain, call token revocation on the server (add a `/v1/me/token` DELETE route for a desktop token to revoke itself; ignore offline errors); if `PRSWT_TOKEN` is set, report "unset PRSWT_TOKEN to sign out".

---

## LOW

- **L1** `sync.rs` `receive` (~142-180): not atomic, so `create_branch` is left behind if `apply` fails; `name == "live"`/empty is accepted; `ttl_minutes` isn't capped. Validate like `sandbox.create` and roll back on error (C1 covers part of this).
- **L2** `sync.rs` (~220-231, ~360-375): no limit on events per branch and changes per event → add `MAX_SYNC_EVENTS` and `changes.len() <= MAX_BATCH`, trim names.
- **L3** `branch.rs` `merge` (~467): `undo.push` without `UNDO_DEPTH` → a shared `push_undo` helper.
- **L4** `cloud.rs` server `watch` (~248): membership is checked only when the stream opens → recheck on keep-alive or abort streams from `remove_member`.
- **L5** `pages.rs` `desktop_consent` (~128-142): show the port and device name, and warn "click Allow only if you just started sign-in from PRSWT".
- **L6** `oidc.rs` / `cloud.rs` `PendingLogins`/`DesktopCodes`: when full, evict the oldest instead of returning 503.
- **L7** `prswt-store/src/lib.rs` `delete_project` (~227-241): two statements in one transaction.
- **L8** Desktop `resume` (~277-290): no token in the keychain → `Done::SignedOut` (otherwise the email from storage hangs around as "signed in"); a `set_password` failure → a status-line message.
- **L9** Desktop `reply` (~577-604): only network errors and 502-504 count as "offline"; a 500 should show the server's message.
- **L10** Desktop `screenshot` (~350-393): ghost "Syncing…" entries make it wait the full 60 s (fixed together with H3 and M12).
- **L11** Desktop `save()` on exit (~151-163): if `pending_branches > 0` or there's a problem, `is_dirty` for a cloud project = true, with the dialog text "changes not yet sent to the cloud; keep them on this computer?"; show a cache-write error.
- **L12** Desktop `watch(id)`: also call it after a successful `sync()` (idempotent), so a project opened without a token gets watched later.

---

## Checked and fine (don't touch)
- Project isolation: every path (`/v1/commands`, `/mcp`, `/v1/events`, `cloud::*`) passes `ctx.access`; `execute` removes other people's projects before running.
- Desktop sign-in: `POST /auth/desktop/allow` is protected by the same-origin check; codes are 256-bit, single-use, 120 s, PKCE S256; `after_login` is not an open redirect.
- Revoking access and tokens takes effect immediately (except open SSE streams, see L4).
- SQL is parameterised; no `unwrap`/panic in the reviewed code; prswt-ui doesn't read the clock.
- `Leave` lives exactly as long as the SSE stream; the job dedupe in the worker is correct; `conflict_text` slices on char boundaries; `ROLES[pick]` is in bounds; the theme is sent only on change.

## Done when
1. Every C/H item fixed, with tests; M items fixed or listed in `docs/gaps.md` with a reason.
2. `./gate.ps1` → GATE PASSED.
3. Manual check against a local server (as in the session log): download a cloud project, push an agent's sandbox, conflicting edits from two copies; a viewer can't discard; the project closes cleanly.
4. Short report: what was fixed, which tests were added, what was deferred and why.
