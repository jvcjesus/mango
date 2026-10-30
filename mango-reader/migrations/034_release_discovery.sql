-- Migration 034: Release-Aware Discovery Lifecycle & Provenance

CREATE TABLE IF NOT EXISTS release_discovery_targets (
    id                      INTEGER PRIMARY KEY AUTOINCREMENT,
    series_id               TEXT NOT NULL,
    manga_id                INTEGER REFERENCES manga(id) ON DELETE CASCADE,
    series_title            TEXT NOT NULL,
    volume_number           REAL NOT NULL,
    market                  TEXT NOT NULL DEFAULT 'en',     -- 'en', 'ja'
    edition                 TEXT NOT NULL DEFAULT 'standard',
    release_date            TEXT NOT NULL,                  -- exact day YYYY-MM-DD
    release_date_precision  TEXT NOT NULL DEFAULT 'day',    -- 'day'
    date_type               TEXT NOT NULL DEFAULT 'published', -- 'published', 'projected'
    status                  TEXT NOT NULL DEFAULT 'scheduled',
    -- 'scheduled': future exact release date
    -- 'releases_today': exact release date reached, ready for discovery
    -- 'discovering': active provider search in progress
    -- 'candidate_available': useful candidate discovered and persisted
    -- 'not_found_retry_wait': searched, no candidate found, waiting for next retry
    -- 'already_owned': ownership check verified volume is present in Komga
    -- 'mapping_review_required': OpenTome and MangaBaka mapping/volume mismatch or ambiguous
    -- 'dormant': max retry attempts exhausted without candidate
    attempts                INTEGER NOT NULL DEFAULT 0,
    max_attempts            INTEGER NOT NULL DEFAULT 7,
    next_attempt_at         INTEGER NOT NULL DEFAULT 0,
    last_attempt_at         INTEGER,
    candidates_count        INTEGER NOT NULL DEFAULT 0,
    top_candidate_id        TEXT,
    top_candidate_title     TEXT,
    last_error              TEXT,
    created_at              INTEGER NOT NULL,
    updated_at              INTEGER NOT NULL,
    UNIQUE(series_id, volume_number, market, edition, release_date)
);

CREATE INDEX IF NOT EXISTS idx_release_discovery_status_next ON release_discovery_targets(status, next_attempt_at);
CREATE INDEX IF NOT EXISTS idx_release_discovery_series ON release_discovery_targets(series_id);
CREATE INDEX IF NOT EXISTS idx_release_discovery_manga ON release_discovery_targets(manga_id);
