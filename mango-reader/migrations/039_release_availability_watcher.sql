-- Migration 039: Release Availability Watcher & Notification Lifecycle (Milestone M6.8.4)
--
-- Provides durable, database-backed availability monitoring for missing manga volumes across
-- providers (Madokami, Nyaa) without enabling automatic acquisition.
--
-- Invariants:
-- - Independent settings: monitored != auto_acquire (auto_acquire defaults strictly to OFF).
-- - State machine tracks release lifecycle, provider check status, candidate fingerprints, and notification.
-- - Komga canonical ownership pre-flight stops monitoring immediately upon ownership.
-- - Deduplicated notifications via durable ntfy outbox.

CREATE TABLE IF NOT EXISTS release_availability_watchers (
    id                          INTEGER PRIMARY KEY AUTOINCREMENT,
    series_id                   TEXT NOT NULL,
    manga_id                    INTEGER REFERENCES manga(id) ON DELETE CASCADE,
    series_title                TEXT NOT NULL,
    volume_number               REAL NOT NULL,
    mangabaka_id                INTEGER,
    collection_id               TEXT,
    work_id                     TEXT,
    market                      TEXT NOT NULL DEFAULT 'en',
    edition                     TEXT NOT NULL DEFAULT 'standard',
    release_date                TEXT,                      -- exact day YYYY-MM-DD if known
    is_released                 INTEGER NOT NULL DEFAULT 0,
    is_owned                    INTEGER NOT NULL DEFAULT 0,
    monitoring_enabled          INTEGER NOT NULL DEFAULT 1,
    notify_on_available         INTEGER NOT NULL DEFAULT 1,
    state                       TEXT NOT NULL DEFAULT 'UPCOMING',
    -- State machine states:
    -- 'UPCOMING': confirmed release date in future
    -- 'RELEASED_AWAITING_PROVIDER': released, awaiting eligible provider candidate
    -- 'ELIGIBLE_CANDIDATE_FOUND': eligible candidate discovered
    -- 'NOTIFIED': notification dispatched via durable outbox
    -- 'ACQUIRED': volume acquired / published
    -- 'COLLECTION_UNAVAILABLE': MangaBaka edition collection unavailable (fails closed)
    -- 'REVIEW_REQUIRED': mapping or title requires operator review
    -- 'PROVIDER_UNAVAILABLE': provider temporarily unreachable (backoff)
    -- 'RATE_LIMITED': provider rate limit encountered (backoff)
    -- 'OWNERSHIP_DETECTED': verified owned in Komga (monitoring complete)
    -- 'MONITORING_DISABLED': operator disabled monitoring for this volume
    last_attempt_at             INTEGER,
    last_checked_at             INTEGER,
    next_check_at               INTEGER NOT NULL DEFAULT 0,
    check_interval_secs         INTEGER NOT NULL DEFAULT 14400, -- default 4 hours
    consecutive_failures        INTEGER NOT NULL DEFAULT 0,
    last_provider_checked       TEXT,
    last_check_status           TEXT,
    last_error                  TEXT,
    top_candidate_id            TEXT,
    top_candidate_provider      TEXT,
    top_candidate_title         TEXT,
    top_candidate_fingerprint   TEXT,
    candidate_count             INTEGER NOT NULL DEFAULT 0,
    notified_at                 INTEGER,
    notification_event_id       INTEGER,
    created_at                  INTEGER NOT NULL,
    updated_at                  INTEGER NOT NULL,
    UNIQUE(series_id, volume_number, market, edition)
);

CREATE INDEX IF NOT EXISTS idx_watchers_state_next ON release_availability_watchers(state, next_check_at);
CREATE INDEX IF NOT EXISTS idx_watchers_series ON release_availability_watchers(series_id);
CREATE INDEX IF NOT EXISTS idx_watchers_manga ON release_availability_watchers(manga_id);

-- Add per-title monitoring and notification toggles to manga table (independent from auto_acquire)
ALTER TABLE manga ADD COLUMN release_monitoring_enabled INTEGER NOT NULL DEFAULT 1;
ALTER TABLE manga ADD COLUMN release_notify_enabled INTEGER NOT NULL DEFAULT 1;
