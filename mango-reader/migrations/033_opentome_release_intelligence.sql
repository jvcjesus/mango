-- Migration 033: OpenTome Release Intelligence Mappings & Provenance

CREATE TABLE IF NOT EXISTS opentome_series_mappings (
    id                      INTEGER PRIMARY KEY AUTOINCREMENT,
    manga_id                INTEGER NOT NULL REFERENCES manga(id) ON DELETE CASCADE,
    series_id               TEXT NOT NULL,
    work_id                 TEXT,
    japanese_series_id      INTEGER,
    english_series_id       INTEGER,
    japanese_tome_id        TEXT,
    english_tome_id         TEXT,
    confidence              TEXT NOT NULL DEFAULT 'unmatched', -- 'exact', 'high', 'ambiguous', 'user_confirmed', 'unmatched', 'rejected'
    mapping_source          TEXT NOT NULL DEFAULT 'auto',     -- 'anilist_id', 'title_exact', 'alias_exact', 'manual'
    title_matched           TEXT,
    last_evaluated_at       INTEGER NOT NULL DEFAULT 0,
    metadata_json           TEXT,                             -- cached OpenTomeSeriesSchedule JSON
    created_at              INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    updated_at              INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    UNIQUE(series_id)
);

CREATE INDEX IF NOT EXISTS idx_opentome_mappings_manga ON opentome_series_mappings(manga_id);
CREATE INDEX IF NOT EXISTS idx_opentome_mappings_series ON opentome_series_mappings(series_id);
CREATE INDEX IF NOT EXISTS idx_opentome_mappings_work ON opentome_series_mappings(work_id);
