-- Login sessions and personal access tokens (AUTH-02/03). Only SHA-256
-- hashes of the secrets are stored; the secrets themselves exist only in
-- the browser cookie or the user's token copy.
CREATE TABLE IF NOT EXISTS sessions (
    token_hash TEXT PRIMARY KEY,
    email      TEXT NOT NULL,
    provider   TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX IF NOT EXISTS sessions_expiry ON sessions(expires_at);

CREATE TABLE IF NOT EXISTS access_tokens (
    id          UUID PRIMARY KEY,
    token_hash  TEXT NOT NULL UNIQUE,
    email       TEXT NOT NULL,
    name        TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ,
    revoked_at  TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS access_tokens_email ON access_tokens(email);
