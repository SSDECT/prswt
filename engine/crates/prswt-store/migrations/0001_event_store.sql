-- PRSWT event store (ADR-0005). Append-only events; state = replay.
CREATE TABLE IF NOT EXISTS projects (
    id          UUID PRIMARY KEY,
    name        TEXT NOT NULL,
    live_branch UUID NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE IF NOT EXISTS branches (
    id         UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    meta       JSONB NOT NULL,
    base       JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS branches_project ON branches(project_id);

CREATE TABLE IF NOT EXISTS events (
    branch_id  UUID   NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
    version    BIGINT NOT NULL CHECK (version > 0),
    project_id UUID   NOT NULL,
    event      JSONB  NOT NULL,
    stored_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (branch_id, version)
);

CREATE TABLE IF NOT EXISTS snapshots (
    branch_id UUID   NOT NULL REFERENCES branches(id) ON DELETE CASCADE,
    version   BIGINT NOT NULL,
    state     JSONB  NOT NULL,
    PRIMARY KEY (branch_id, version)
);

CREATE TABLE IF NOT EXISTS simulation_runs (
    id         UUID PRIMARY KEY,
    project_id UUID NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    branch_id  UUID NOT NULL,
    summary    JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
