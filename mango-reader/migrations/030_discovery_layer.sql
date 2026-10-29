-- Migration 030: Discovery Layer & Logical Event Identity

CREATE TABLE reconciliation_events_v2 (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    series_id TEXT NOT NULL,
    mangabaka_id INTEGER NULL,
    mangabaka_volume_id TEXT NULL,
    series_title TEXT NOT NULL,
    edition TEXT NOT NULL DEFAULT 'standard',
    volume_number REAL NULL,
    event_type TEXT NOT NULL,
    top_candidate_id TEXT NULL,
    candidate_count INTEGER NOT NULL DEFAULT 0,
    message TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    notified_at INTEGER NULL,
    UNIQUE(series_id, volume_number, event_type)
);

INSERT OR IGNORE INTO reconciliation_events_v2 (id, series_id, mangabaka_id, series_title, edition, volume_number, event_type, message, created_at, notified_at)
SELECT id, 'title:' || lower(series_title), mangabaka_id, series_title, 'standard', volume_number, event_type, message, created_at, notified_at
FROM reconciliation_events;

DROP TABLE reconciliation_events;
ALTER TABLE reconciliation_events_v2 RENAME TO reconciliation_events;

CREATE INDEX idx_reconciliation_events_series ON reconciliation_events(series_id);
CREATE INDEX idx_reconciliation_events_notified ON reconciliation_events(notified_at);

-- Discovered release candidates per logical target
CREATE TABLE discovered_release_candidates (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    series_id TEXT NOT NULL,
    volume_number REAL NOT NULL,
    candidate_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    provider_item_id TEXT NOT NULL,
    title TEXT NOT NULL,
    raw_title TEXT NOT NULL,
    match_type TEXT NOT NULL,
    size_bytes INTEGER NULL,
    uploader TEXT NULL,
    is_trusted INTEGER NOT NULL DEFAULT 0,
    is_digital INTEGER NOT NULL DEFAULT 0,
    score REAL NOT NULL,
    score_explanation TEXT NOT NULL,
    download_url TEXT NULL,
    magnet_url TEXT NULL,
    info_hash TEXT NULL,
    seeders INTEGER NULL,
    leechers INTEGER NULL,
    published_at INTEGER NULL,
    discovered_at INTEGER NOT NULL,
    UNIQUE(series_id, volume_number, candidate_id)
);
CREATE INDEX idx_discovered_candidates_target ON discovered_release_candidates(series_id, volume_number);

-- Status of release discovery per logical missing volume
CREATE TABLE volume_discovery_status (
    series_id TEXT NOT NULL,
    volume_number REAL NOT NULL,
    last_discovered_at INTEGER NOT NULL,
    status TEXT NOT NULL,
    candidate_count INTEGER NOT NULL DEFAULT 0,
    top_candidate_id TEXT NULL,
    top_candidate_title TEXT NULL,
    top_candidate_provider TEXT NULL,
    top_candidate_score REAL NULL,
    providers_checked TEXT NOT NULL,
    provider_errors TEXT NULL,
    PRIMARY KEY(series_id, volume_number)
);
CREATE INDEX idx_volume_discovery_status_series ON volume_discovery_status(series_id);
