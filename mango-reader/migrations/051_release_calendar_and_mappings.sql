-- Migration 051: Release Calendar, Schedule Persistence & Nullable Manga FK in OpenTome Mappings

PRAGMA foreign_keys=OFF;

CREATE TABLE opentome_series_mappings_v2 (
    id                      INTEGER PRIMARY KEY AUTOINCREMENT,
    manga_id                INTEGER NULL REFERENCES manga(id) ON DELETE CASCADE,
    series_id               TEXT NOT NULL,
    work_id                 TEXT,
    japanese_series_id      INTEGER,
    english_series_id       INTEGER,
    japanese_tome_id        TEXT,
    english_tome_id         TEXT,
    confidence              TEXT NOT NULL DEFAULT 'unmatched',
    mapping_source          TEXT NOT NULL DEFAULT 'auto',
    title_matched           TEXT,
    last_evaluated_at       INTEGER NOT NULL DEFAULT 0,
    metadata_json           TEXT,
    created_at              INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    updated_at              INTEGER NOT NULL DEFAULT (strftime('%s','now')),
    UNIQUE(series_id)
);

INSERT INTO opentome_series_mappings_v2 (
    id, manga_id, series_id, work_id, japanese_series_id, english_series_id,
    japanese_tome_id, english_tome_id, confidence, mapping_source,
    title_matched, last_evaluated_at, metadata_json, created_at, updated_at
)
SELECT id, manga_id, series_id, work_id, japanese_series_id, english_series_id,
    japanese_tome_id, english_tome_id, confidence, mapping_source,
    title_matched, last_evaluated_at, metadata_json, created_at, updated_at
FROM opentome_series_mappings;

DROP TABLE opentome_series_mappings;
ALTER TABLE opentome_series_mappings_v2 RENAME TO opentome_series_mappings;

CREATE INDEX IF NOT EXISTS idx_opentome_mappings_manga ON opentome_series_mappings(manga_id);
CREATE INDEX IF NOT EXISTS idx_opentome_mappings_series ON opentome_series_mappings(series_id);
CREATE INDEX IF NOT EXISTS idx_opentome_mappings_work ON opentome_series_mappings(work_id);

ALTER TABLE series_reconciliations ADD COLUMN upcoming_releases_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE series_reconciliations ADD COLUMN last_schedule_refresh_at INTEGER NOT NULL DEFAULT 0;

PRAGMA foreign_keys=ON;
