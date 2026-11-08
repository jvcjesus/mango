-- Migration 047: Followed-Series Monitoring, Review Queue & Automation Policy Engine (Milestone M10.1 + M11)

-- 1. Proposed acquisitions review queue (Mode B: Review Before Acquisition)
CREATE TABLE IF NOT EXISTS acquisition_review_queue (
    id                          INTEGER PRIMARY KEY AUTOINCREMENT,
    manga_id                    INTEGER REFERENCES manga(id) ON DELETE CASCADE,
    series_id                   TEXT NOT NULL,
    series_title                TEXT NOT NULL,
    volume_number               REAL NOT NULL,
    recommended_candidate_id    TEXT NOT NULL,
    provider                    TEXT NOT NULL,
    title                       TEXT NOT NULL,
    raw_title                   TEXT NOT NULL,
    quality_score               REAL NOT NULL,
    confidence                  TEXT NOT NULL DEFAULT 'high',
    confidence_reason           TEXT,
    file_size_bytes             INTEGER,
    publication_destination     TEXT,
    status                      TEXT NOT NULL DEFAULT 'pending', -- 'pending', 'approved', 'rejected', 'skipped'
    status_reason               TEXT,
    alternatives_json           TEXT,
    ranking_breakdown_json      TEXT,
    created_at                  INTEGER NOT NULL,
    updated_at                  INTEGER NOT NULL,
    UNIQUE(series_id, volume_number)
);

CREATE INDEX IF NOT EXISTS idx_review_queue_status ON acquisition_review_queue(status);
CREATE INDEX IF NOT EXISTS idx_review_queue_series ON acquisition_review_queue(series_id);

-- 2. Monitoring activity and decision audit history
CREATE TABLE IF NOT EXISTS monitoring_activity_history (
    id                          INTEGER PRIMARY KEY AUTOINCREMENT,
    manga_id                    INTEGER REFERENCES manga(id) ON DELETE SET NULL,
    series_title                TEXT NOT NULL,
    volume_number               REAL NOT NULL,
    action                      TEXT NOT NULL, -- 'discovered', 'plan_proposed', 'approved', 'rejected', 'auto_acquired', 'simulated', 'failed'
    source                      TEXT NOT NULL,
    confidence                  TEXT NOT NULL DEFAULT 'high',
    quality_score               REAL,
    decision_code               TEXT,
    details                     TEXT NOT NULL,
    created_at                  INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_monitoring_activity_time ON monitoring_activity_history(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_monitoring_activity_manga ON monitoring_activity_history(manga_id);

-- 3. Per-series automation overrides (optional overrides of global automation mode)
ALTER TABLE manga ADD COLUMN automation_mode TEXT DEFAULT NULL;
ALTER TABLE manga ADD COLUMN monitored_tracking_statuses TEXT DEFAULT NULL;
