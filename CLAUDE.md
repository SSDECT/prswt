# PRSWT — instructions for agents

PRSWT is a clean-room, Rust-native, MCP-first project-controls and risk engine targeting Oracle Primavera P6 parity (scheduling rigor) and beyond (stochastic risk, sandboxes, agent control). Conventions follow the ArtCraft "Craft" suite; see `vault/04-Decisions/Audit-ArtCraft.md`.

## Start every session here
1. Read `vault/01-Plan/Master-Plan.md`, `vault/03-Scorecard/Scorecard.md` and the latest `vault/06-Sessions/` log.
2. Work autonomously. Ask only when blocked (admin rights, credentials).
3. At the end: tick the Scorecard, append the session log, and update `ROADMAP.md` / `docs/*` timestamps when work lands.

## Never crash (outranks feature work)
- No `unwrap/expect/panic!/todo!/unimplemented!/unreachable!` outside tests (crate-level `deny`); `unsafe` is forbidden.
- Errors are `Result<_, EngineError>` with `?`. Input-derived numbers are hostile: `get()`, checked or saturating arithmetic, capped allocations, bounded recursion. Plain indexing is allowed only on engine-generated dense indices.
- `execute` catches escaped panics and restores state. Every crash fix ships with a regression test.

## Non-negotiables
- **Everything is a command**: user-visible behaviour = a `CommandSpec` in `prswt-app` (id after P6's menus, params doc, run) + tests. UI, CLI, control channel and MCP all reach it.
- **Layering** (`cargo xtask layers`): L0 `core`, `calendar` → L1 `cpm`, `resource`, `evm`, `risk` → L2 `state` → L3 `store`, `app` → L4 `mcp`, `api`, `ui` → L5 `apps/*`. Math crates do no IO, read no clock and stay wasm-safe.
- **Clean-room**: P6 / MS Project behaviour comes from public documentation and black-box observation only. No vendor assets, artwork, copied text or vendor-produced fixtures. Build samples and fixtures in code.
- **No placeholder math**: every Scorecard item has tests against a hand-worked oracle in `vault/05-Math/` (plus proptest invariants).
- Time = integer minutes (`Instant`, `WorkMinutes`); money = `Decimal`; RNG = ChaCha8 with one stream per iteration.
- **Gate**: `cargo xtask ci` (or `./gate.ps1`): fmt, clippy -D warnings, nextest, doctests, deny, layers.
- **Secrets**: Bitwarden only (`bw`, folder `PRSWT`, items `prswt/<name>`), session from the `BW_SESSION` user env var. Never print or write secret values.
- Commit only when the user asks; one Scorecard id per commit (`CPM-04: SS/FF/SF links`).

## Documentation (enforced)
- Every module starts with a `//!` overview: purpose, how it fits, where its spec/oracle lives.
- Every public item (types, fields, variants, functions, constants) has a `///` doc saying what it is and why, with units, invariants and error cases. `missing_docs` is denied workspace-wide, and `cargo doc` runs with `-D warnings`.
- Put docs above attributes. Non-obvious private logic gets a short *why* comment; don't restate the code.

## Code layout (top-to-bottom reading)
Constants → primary public type → supporting types → impl (constructors, public, private; callers above callees). Tests: cases first, helpers last; sub-modules for 2+ related tests.
