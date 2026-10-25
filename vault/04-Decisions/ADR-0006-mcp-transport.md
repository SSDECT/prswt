# ADR-0006 — MCP: hand-written JSON-RPC over stdio (revised 2026-10-23)
**Status:** accepted; supersedes the original `rmcp` choice after [[Audit-ArtCraft]].
**Decision:** `prswt-mcp` is a sync library: `Server::handle_line(&str) -> Option<String>` speaking newline-delimited JSON-RPC 2.0 (protocol 2025-06-18; older clients are answered in their own version). It implements `initialize`, `tools/*`, `resources/*` and `prompts/*`. Two backends: `Headless` (in-process `prswt-app`) and `Remote` (`--connect <port>` to a running app's localhost JSON-lines control channel). `prswt-cli mcp` hosts it.
**Why:** no async runtime in the agent path; every message is unit-testable and fuzzable through `handle_line`; the same pattern as GridCraft.
**Consequences:** we maintain the protocol surface ourselves (small: about 10 methods). Streamable HTTP can be added later in `prswt-api`.
