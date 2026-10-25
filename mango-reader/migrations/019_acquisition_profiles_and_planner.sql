-- Acquisition profiles, source intelligence, and missing-content planner persistence

CREATE TABLE IF NOT EXISTS acquisition_profiles (
    id                                  TEXT PRIMARY KEY,
    name                                TEXT NOT NULL,
    is_default                          INTEGER NOT NULL DEFAULT 0,
    language                            TEXT NOT NULL DEFAULT 'en',
    preferred_release_type              TEXT NOT NULL DEFAULT 'volume',
    source_selection                    TEXT NOT NULL DEFAULT 'smart',
    preferred_sources                   TEXT NOT NULL DEFAULT '["nyaa"]',
    preferred_uploaders                 TEXT NOT NULL DEFAULT '[]',
    min_seeders                         INTEGER NOT NULL DEFAULT 1,
    prefer_digital                      INTEGER NOT NULL DEFAULT 1,
    trusted_only                        INTEGER NOT NULL DEFAULT 0,
    collection_policy                   TEXT NOT NULL DEFAULT 'smart',
    max_unnecessary_duplicate_coverage  INTEGER NOT NULL DEFAULT 3,
    max_acquisition_size_bytes          INTEGER,
    max_plan_size_bytes                 INTEGER,
    quality_preference                  TEXT NOT NULL DEFAULT 'best',
    allow_mixed_collections             INTEGER NOT NULL DEFAULT 0,
    allow_zero_seeders                  TEXT NOT NULL DEFAULT 'no',
    allow_unsupported_editions          INTEGER NOT NULL DEFAULT 0,
    auto_acquire                        INTEGER NOT NULL DEFAULT 0,
    created_at                          INTEGER NOT NULL,
    updated_at                          INTEGER NOT NULL
);

-- Default profile inserted automatically
INSERT OR IGNORE INTO acquisition_profiles (
    id, name, is_default, language, preferred_release_type, source_selection,
    preferred_sources, preferred_uploaders, min_seeders, prefer_digital,
    trusted_only, collection_policy, max_unnecessary_duplicate_coverage,
    quality_preference, allow_mixed_collections, allow_zero_seeders,
    allow_unsupported_editions, auto_acquire, created_at, updated_at
) VALUES (
    'default', 'Default Manga', 1, 'en', 'volume', 'smart',
    '["nyaa", "mangadot"]', '[]', 1, 1,
    0, 'smart', 3,
    'best', 0, 'no',
    0, 0, CAST(strftime('%s', 'now') AS INTEGER), CAST(strftime('%s', 'now') AS INTEGER)
);

-- Per-series acquisition profile assignment
ALTER TABLE manga ADD COLUMN acquisition_profile_id TEXT REFERENCES acquisition_profiles(id);
CREATE INDEX IF NOT EXISTS idx_manga_acquisition_profile ON manga(acquisition_profile_id);

-- Persisted acquisition plans
CREATE TABLE IF NOT EXISTS acquisition_plans (
    id                          TEXT PRIMARY KEY,
    manga_id                    INTEGER NOT NULL REFERENCES manga(id) ON DELETE CASCADE,
    profile_id                  TEXT NOT NULL REFERENCES acquisition_profiles(id),
    missing_units_json          TEXT NOT NULL,
    selected_candidates_json    TEXT NOT NULL,
    alternative_candidates_json TEXT NOT NULL DEFAULT '[]',
    explanations_json           TEXT NOT NULL DEFAULT '[]',
    confidence                  TEXT NOT NULL,
    state                       TEXT NOT NULL,
    stale_reason                TEXT,
    created_at                  INTEGER NOT NULL,
    refreshed_at                INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_acq_plans_manga ON acquisition_plans(manga_id);

-- Source health, capability and reliability intelligence
CREATE TABLE IF NOT EXISTS source_intelligence (
    source_id                   TEXT PRIMARY KEY,
    name                        TEXT NOT NULL,
    supports_volumes            INTEGER NOT NULL DEFAULT 1,
    supports_chapters           INTEGER NOT NULL DEFAULT 0,
    supports_collections        INTEGER NOT NULL DEFAULT 0,
    requires_external_backend   INTEGER NOT NULL DEFAULT 0,
    status                      TEXT NOT NULL DEFAULT 'healthy',
    success_count               INTEGER NOT NULL DEFAULT 0,
    failure_count               INTEGER NOT NULL DEFAULT 0,
    last_response_time_ms       INTEGER,
    last_error                  TEXT,
    last_success_at             INTEGER,
    exact_match_count           INTEGER NOT NULL DEFAULT 0,
    total_candidates_found      INTEGER NOT NULL DEFAULT 0,
    uploader_stats_json         TEXT NOT NULL DEFAULT '{}',
    updated_at                  INTEGER NOT NULL
);

INSERT OR IGNORE INTO source_intelligence (
    source_id, name, supports_volumes, supports_chapters, supports_collections,
    requires_external_backend, status, updated_at
) VALUES 
('nyaa', 'Nyaa', 1, 1, 1, 1, 'healthy', CAST(strftime('%s', 'now') AS INTEGER)),
('mangadot', 'MangaDot', 1, 1, 0, 0, 'healthy', CAST(strftime('%s', 'now') AS INTEGER));
