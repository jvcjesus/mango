-- Migration 050: Autonomous Acquisition Orchestrator, Idempotent State Machine & Self-Healing Pipeline (Milestone M14)

-- 1. Acquisition Lifecycle State Machine and Idempotency Leases
CREATE TABLE IF NOT EXISTS acquisition_lifecycle (
    id                          INTEGER PRIMARY KEY AUTOINCREMENT,
    idempotency_key             TEXT NOT NULL UNIQUE,
    series_id                   TEXT NOT NULL,
    manga_id                    INTEGER REFERENCES manga(id) ON DELETE CASCADE,
    series_title                TEXT NOT NULL,
    volume_number               REAL NOT NULL,
    edition                     TEXT NOT NULL DEFAULT 'standard',
    state                       TEXT NOT NULL DEFAULT 'DISCOVERED',
    candidate_id                TEXT,
    provider                    TEXT,
    release_title               TEXT,
    chapter_id                  INTEGER REFERENCES chapters(id) ON DELETE SET NULL,
    download_job_id             INTEGER,
    publication_receipt_id      TEXT,
    lease_token                 TEXT,
    lease_expires_at            INTEGER,
    retry_count                 INTEGER NOT NULL DEFAULT 0,
    max_retries                 INTEGER NOT NULL DEFAULT 3,
    next_retry_at               INTEGER,
    failure_classification      TEXT,
    last_error                  TEXT,
    created_at                  INTEGER NOT NULL,
    updated_at                  INTEGER NOT NULL,
    completed_at                INTEGER
);

CREATE INDEX IF NOT EXISTS idx_acquisition_lifecycle_series_vol ON acquisition_lifecycle(series_id, volume_number);
CREATE INDEX IF NOT EXISTS idx_acquisition_lifecycle_state ON acquisition_lifecycle(state);
CREATE INDEX IF NOT EXISTS idx_acquisition_lifecycle_leases ON acquisition_lifecycle(lease_expires_at);
CREATE INDEX IF NOT EXISTS idx_acquisition_lifecycle_retries ON acquisition_lifecycle(next_retry_at);

-- 2. Per-Series Automation Overrides on manga table
ALTER TABLE manga ADD COLUMN automation_override TEXT NOT NULL DEFAULT 'inherit';

-- 3. Monitoring activity history stage tracking
ALTER TABLE monitoring_activity_history ADD COLUMN stage TEXT DEFAULT NULL;
ALTER TABLE monitoring_activity_history ADD COLUMN duration_ms INTEGER DEFAULT NULL;
