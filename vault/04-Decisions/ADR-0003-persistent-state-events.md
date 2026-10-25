# ADR-0003 — Persistent immutable state + event sourcing
**Status:** accepted
**Context:** Sandboxes must fork in O(1), diff cheaply and merge with conflict detection; audit is required.
**Decision:** `ProjectState` on `im` HAMT/B-tree maps (structural sharing). Every change is Command → Event, carrying `touched` (entity, field) sets. Branch = fork pointer + events. Merge = rebase with field-level conflicts.
**Consequences:** ~1.5–3× memory per entity vs plain maps. Hot math runs on the compiled dense model, not on `im`.
