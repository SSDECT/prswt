# ADR-0007 — Adopt the ArtCraft/GridCraft "Craft" engineering conventions
**Status:** accepted (2026-10-23, user asked to follow ArtCraft's approach). Evidence: [[Audit-ArtCraft]].
**Decision:**
1. **Everything is a command.** A `CommandSpec` registry in `prswt-app`; ids follow P6 actions. `execute(id, params)` is the single write path for UI, CLI, control channel and MCP.
2. **Never crash.** Clippy `deny(unwrap_used, expect_used, panic, todo, unimplemented, unreachable)` outside tests; `unsafe` forbidden; hostile-input handling; bounded recursion; a `catch_unwind` guard in `execute`; fuzz every command (empty/garbage params) and random command sequences against invariants.
3. **Layering** enforced by `cargo xtask layers`. **One gate**: `cargo xtask ci`.
4. **Parity is measured**: a P6 catalog → `docs/parity-checklist.md` via `cargo xtask parity`.
5. **Progress docs**: `ROADMAP.md`, `docs/{architecture,target-app-parity,gaps}.md`, each with timestamp lines and a revision history.
6. **Clean-room** with respect to Oracle Primavera P6 and MS Project.
7. Samples and fixtures are built in code.
**Consequences:** extra discipline on error paths; plus a CLI, xtask and the parity catalog to build.
