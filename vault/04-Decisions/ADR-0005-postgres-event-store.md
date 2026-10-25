# ADR-0005 — PostgreSQL event store, in-memory graph
**Status:** accepted
**Context:** Graph traversal is the hot path. The DB is needed for durability, multi-user access and audit.
**Options:** Neo4j/graph DB · SQLite · PostgreSQL.
**Decision:** PostgreSQL 17 (Docker locally) with append-only `events` (unique stream_id+version), `snapshots`, `simulation_runs`. Access via `sqlx`. Credentials from Bitwarden `prswt/postgres` at runtime.
**Consequences:** the graph DB's query language is not available. Graph queries are served by the engine through MCP resources instead.
