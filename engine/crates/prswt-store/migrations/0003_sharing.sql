-- SYNC-01/04 (ADR-0010): who may use which project, token kinds and
-- per-user settings. No foreign key to projects: membership is written
-- the moment a project is created, before its first sync to this table's
-- neighbours; deleting a project removes its members explicitly.
CREATE TABLE IF NOT EXISTS project_members (
    project_id UUID NOT NULL,
    email      TEXT NOT NULL,
    role       TEXT NOT NULL CHECK (role IN ('viewer', 'editor', 'owner')),
    added_by   TEXT NOT NULL,
    added_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (project_id, email)
);
CREATE INDEX IF NOT EXISTS project_members_email ON project_members(email);

-- agent: sandbox-only agent (default); read: read-only agent;
-- desktop: the signed-in desktop app, acting as the person.
ALTER TABLE access_tokens ADD COLUMN IF NOT EXISTS kind TEXT NOT NULL DEFAULT 'agent'
    CHECK (kind IN ('agent', 'read', 'desktop'));

CREATE TABLE IF NOT EXISTS user_settings (
    email      TEXT PRIMARY KEY,
    settings   JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
