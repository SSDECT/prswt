-- One-time server tasks that must not repeat on every start (for example
-- adopting projects stored before sharing existed).
CREATE TABLE IF NOT EXISTS server_flags (
    name    TEXT PRIMARY KEY,
    done_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
