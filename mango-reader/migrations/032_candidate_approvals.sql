-- Migration 032: Candidate approvals provenance table for M6.4 explicit manual approval

CREATE TABLE IF NOT EXISTS candidate_approvals (
    id                      INTEGER PRIMARY KEY AUTOINCREMENT,
    manga_id                INTEGER NOT NULL REFERENCES manga(id) ON DELETE CASCADE,
    series_id               TEXT NOT NULL,
    volume_number           REAL NOT NULL,
    candidate_id            TEXT NOT NULL,
    provider                TEXT NOT NULL,
    provider_release_id     TEXT NOT NULL,
    match_type              TEXT NOT NULL,
    approved_at             INTEGER NOT NULL,
    execution_id            TEXT,
    chapter_id              INTEGER,
    status                  TEXT NOT NULL DEFAULT 'approved'
);

CREATE INDEX IF NOT EXISTS idx_candidate_approvals_target ON candidate_approvals(series_id, volume_number);
CREATE INDEX IF NOT EXISTS idx_candidate_approvals_cand ON candidate_approvals(candidate_id);
