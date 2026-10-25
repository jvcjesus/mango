-- Acquisition executions and execution items for approved plan execution and verification pipeline

CREATE TABLE IF NOT EXISTS acquisition_executions (
    id                              TEXT PRIMARY KEY,
    plan_id                         TEXT NOT NULL REFERENCES acquisition_plans(id),
    manga_id                        INTEGER NOT NULL REFERENCES manga(id) ON DELETE CASCADE,
    profile_id                      TEXT NOT NULL,
    title                           TEXT NOT NULL,
    status                          TEXT NOT NULL, -- APPROVED, PREFLIGHT, REVALIDATING, QUEUED, DOWNLOADING, VERIFYING, PUBLISHING, RECONCILING, COMPLETED, PARTIAL, FAILED, CANCELLED, STALE
    plan_snapshot_json              TEXT NOT NULL,
    missing_content_snapshot_json   TEXT NOT NULL,
    candidates_snapshot_json        TEXT NOT NULL,
    expected_units_json             TEXT NOT NULL,
    expected_total_bytes            INTEGER NOT NULL,
    actual_downloaded_bytes         INTEGER,
    failure_reason                  TEXT,
    cancellation_reason             TEXT,
    summary                         TEXT,
    created_at                      INTEGER NOT NULL,
    approved_at                     INTEGER NOT NULL,
    started_at                      INTEGER,
    completed_at                    INTEGER,
    updated_at                      INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_acq_exec_manga ON acquisition_executions(manga_id);
CREATE INDEX IF NOT EXISTS idx_acq_exec_status ON acquisition_executions(status);
CREATE INDEX IF NOT EXISTS idx_acq_exec_created ON acquisition_executions(created_at DESC);

CREATE TABLE IF NOT EXISTS acquisition_execution_items (
    id                              TEXT PRIMARY KEY,
    execution_id                    TEXT NOT NULL REFERENCES acquisition_executions(id) ON DELETE CASCADE,
    provider                        TEXT NOT NULL,
    provider_candidate_id           TEXT NOT NULL,
    frozen_candidate_json           TEXT NOT NULL,
    expected_units_json             TEXT NOT NULL,
    expected_size                   INTEGER NOT NULL,
    status                          TEXT NOT NULL, -- PENDING, PREFLIGHT, REVALIDATING, QUEUED, DOWNLOADING, VERIFYING, PUBLISHING, RECONCILING, COMPLETED, PARTIAL, FAILED, SKIPPED, CANCELLED
    download_job_id                 INTEGER,
    verification_state              TEXT,
    publication_state               TEXT,
    retry_count                     INTEGER NOT NULL DEFAULT 0,
    error_message                   TEXT,
    created_at                      INTEGER NOT NULL,
    updated_at                      INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_acq_exec_items_exec ON acquisition_execution_items(execution_id);
CREATE INDEX IF NOT EXISTS idx_acq_exec_items_job ON acquisition_execution_items(download_job_id);
CREATE INDEX IF NOT EXISTS idx_acq_exec_items_status ON acquisition_execution_items(status);

-- Add execution references to downloads queue for idempotent coordination
ALTER TABLE downloads ADD COLUMN execution_id TEXT;
ALTER TABLE downloads ADD COLUMN execution_item_id TEXT;
CREATE INDEX IF NOT EXISTS idx_downloads_exec_item ON downloads(execution_item_id);
