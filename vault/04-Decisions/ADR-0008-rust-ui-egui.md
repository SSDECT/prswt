# ADR-0008 — Rust-only UI with egui (replaces React/TS from SPEC-01 §2.8)
**Status:** accepted
**Context:** the ArtCraft suite ships egui/eframe native apps plus WASM web builds with no hand-written JS/TS. A thin UI that calls `execute(id, params)` keeps the UI a fourth equal client alongside MCP, CLI and the control channel.
**Decision:** `prswt-ui` (egui widgets: Gantt, network, risk dashboards) + `apps/prswt` (eframe desktop). Math/state crates must stay wasm-safe (no clock or threads at the API boundary; rayon is optional behind a feature).
**Consequences:** one language end to end; the Gantt is custom-painted on the egui painter; the web build comes later via trunk.
