-- Migration 043: Explicit execution authorizations for manual acquisition workflow

CREATE TABLE IF NOT EXISTS acquisition_execution_authorizations (
    id                      TEXT PRIMARY KEY,
    manga_id                INTEGER NOT NULL REFERENCES manga(id) ON DELETE CASCADE,
    mangabaka_id            INTEGER,
    series_id               TEXT NOT NULL,
    edition                 TEXT NOT NULL,
    volume_number           REAL NOT NULL,
    candidate_id            TEXT NOT NULL,
    provider                TEXT NOT NULL,
    backend                 TEXT NOT NULL,
    user_id                 INTEGER,
    created_at              INTEGER NOT NULL,
    expires_at              INTEGER NOT NULL,
    consumed_at             INTEGER,
    status                  TEXT NOT NULL DEFAULT 'authorized'
);

CREATE INDEX IF NOT EXISTS idx_acq_exec_auth_token ON acquisition_execution_authorizations(id);
CREATE INDEX IF NOT EXISTS idx_acq_exec_auth_target ON acquisition_execution_authorizations(manga_id, volume_number);
