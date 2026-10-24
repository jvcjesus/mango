-- Tracking & reading progress synchronization foundation.

CREATE TABLE IF NOT EXISTS tracker_mappings (
    komga_series_id TEXT NOT NULL,
    tracker         TEXT NOT NULL, -- 'anilist', 'mangabaka'
    tracker_id      TEXT NOT NULL, -- e.g. AniList mediaId or MangaBaka seriesId
    tracker_title   TEXT,
    manual_override INTEGER NOT NULL DEFAULT 0,
    updated_at      INTEGER NOT NULL,
    PRIMARY KEY (komga_series_id, tracker)
);

CREATE TABLE IF NOT EXISTS sync_records (
    id                 TEXT PRIMARY KEY,
    timestamp          INTEGER NOT NULL,
    komga_series_id    TEXT NOT NULL,
    komga_series_title TEXT NOT NULL,
    tracker            TEXT NOT NULL,
    tracker_media_id   TEXT,
    old_progress       REAL,
    new_progress       REAL,
    status_change      TEXT,
    category           TEXT NOT NULL,
    message            TEXT NOT NULL,
    is_dry_run         INTEGER NOT NULL DEFAULT 1
);

CREATE INDEX IF NOT EXISTS sync_records_time ON sync_records(timestamp DESC);

CREATE TABLE IF NOT EXISTS tracked_catalog (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    manga_id           INTEGER REFERENCES manga(id) ON DELETE SET NULL,
    komga_series_id    TEXT,
    title              TEXT NOT NULL,
    origins            TEXT NOT NULL DEFAULT '["mango"]',
    auto_acquire       INTEGER NOT NULL DEFAULT 0,
    tracked            INTEGER NOT NULL DEFAULT 1,
    created_at         INTEGER NOT NULL,
    updated_at         INTEGER NOT NULL
);
