-- Tracked lists cache and refresh metadata for unified catalog.

CREATE TABLE IF NOT EXISTS tracker_list_entries (
    tracker            TEXT NOT NULL, -- 'anilist', 'mangabaka'
    tracker_id         TEXT NOT NULL,
    title              TEXT NOT NULL,
    cover_url          TEXT,
    status             TEXT NOT NULL, -- native provider status string ('CURRENT', 'plan_to_read', etc.)
    normalized_status  TEXT NOT NULL, -- 'reading', 'planning', 'completed', 'paused', 'dropped'
    progress_chapter   REAL,
    progress_volume    REAL,
    total_chapters     INTEGER,
    total_volumes      INTEGER,
    format             TEXT,
    updated_at         INTEGER NOT NULL,
    raw_data           TEXT,
    PRIMARY KEY (tracker, tracker_id)
);

CREATE INDEX IF NOT EXISTS tracker_list_entries_norm_status ON tracker_list_entries(normalized_status);
CREATE INDEX IF NOT EXISTS tracker_list_entries_updated ON tracker_list_entries(updated_at DESC);

CREATE TABLE IF NOT EXISTS tracker_list_refresh_status (
    tracker            TEXT PRIMARY KEY,
    last_refreshed_at  INTEGER NOT NULL,
    item_count         INTEGER NOT NULL DEFAULT 0,
    status             TEXT NOT NULL DEFAULT 'ok', -- 'ok', 'error', 'rate_limited', 'unauthorized'
    error_message      TEXT
);
