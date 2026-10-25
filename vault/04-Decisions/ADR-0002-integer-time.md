# ADR-0002 — Integer minute time model
**Status:** accepted
**Context:** Floating-point days drift, and P6 parity needs exact calendar arithmetic.
**Decision:** `Instant` = i64 minutes since 2000-01-01 (project-local). `WorkMinutes` = i64. Calendars keep sorted working intervals with prefix sums, so `add_work`/`work_between` are O(log n) binary searches.
**Consequences:** time zones are out of scope (one project time zone). Sub-minute precision is not supported (not needed).
