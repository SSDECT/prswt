# ADR-0001 — Rust for the core engine
**Status:** accepted (2026-10-23, user approved)
**Context:** CPM recalcs plus Monte Carlo with thousands of full network passes; the MCP server and API must share the same logic.
**Options:** Rust · TypeScript · C++ · Go · Python + NumPy.
**Decision:** Rust. Native speed with no GC, `rayon` parallelism without data races, newtypes for unit safety, one binary for engine + MCP (`rmcp`) + HTTP (`axum`).
**Consequences:** slower iteration than TS. Mitigated by pure math crates that compile and test fast. Frontend stays TS.
See [[01-Stack]].
