-- M6.10: Guarded Historical Backlog Acquisition
-- Tracks bounded backlog acquisition jobs, user confirmations, execution states, and audit trails.

CREATE TABLE IF NOT EXISTS backlog_acquisition_jobs (
    id                      TEXT PRIMARY KEY,
    manga_id                INTEGER NOT NULL REFERENCES manga(id) ON DELETE CASCADE,
    series_title            TEXT NOT NULL,
    scope                   TEXT NOT NULL, -- 'ALL_MISSING', 'RANGE', 'EXPLICIT_VOLUMES'
    requested_volumes_json  TEXT NOT NULL, -- JSON array of requested volumes e.g. [1, 2, 3]
    batch_size              INTEGER NOT NULL DEFAULT 5,
    is_dry_run              INTEGER NOT NULL DEFAULT 1,
    state                   TEXT NOT NULL DEFAULT 'PLANNED', -- 'PLANNED', 'COMPLETED', 'PARTIAL', 'FAILED', 'CANCELLED'
    total_requested         INTEGER NOT NULL DEFAULT 0,
    total_eligible          INTEGER NOT NULL DEFAULT 0,
    total_already_owned     INTEGER NOT NULL DEFAULT 0,
    total_queued            INTEGER NOT NULL DEFAULT 0,
    total_failed            INTEGER NOT NULL DEFAULT 0,
    items_json              TEXT NOT NULL DEFAULT '[]',
    confirmed_by_user       INTEGER NOT NULL DEFAULT 0,
    error                   TEXT,
    created_at              INTEGER NOT NULL,
    updated_at              INTEGER NOT NULL,
    completed_at            INTEGER
);

CREATE INDEX IF NOT EXISTS idx_backlog_jobs_manga ON backlog_acquisition_jobs(manga_id);
CREATE INDEX IF NOT EXISTS idx_backlog_jobs_state ON backlog_acquisition_jobs(state);
CREATE INDEX IF NOT EXISTS idx_backlog_jobs_created ON backlog_acquisition_jobs(created_at DESC);
