-- Migration 022: MangaBaka series catalog cache and existing-library normalization history

CREATE TABLE IF NOT EXISTS mangabaka_series_cache (
    series_id       INTEGER PRIMARY KEY,
    title           TEXT NOT NULL,
    english_title   TEXT,
    romaji_title    TEXT,
    native_title    TEXT,
    aliases         TEXT NOT NULL DEFAULT '[]',
    cover_url       TEXT,
    authors         TEXT NOT NULL DEFAULT '[]',
    artists         TEXT NOT NULL DEFAULT '[]',
    publisher       TEXT,
    genres          TEXT NOT NULL DEFAULT '[]',
    tags            TEXT NOT NULL DEFAULT '[]',
    summary         TEXT,
    total_volumes   INTEGER,
    status          TEXT,
    web_url         TEXT,
    raw_json        TEXT,
    updated_at      INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_mb_cache_title ON mangabaka_series_cache(title);

CREATE TABLE IF NOT EXISTS normalization_history (
    id                      TEXT PRIMARY KEY,
    batch_id                TEXT NOT NULL,
    canonical_path          TEXT NOT NULL,
    series_title            TEXT NOT NULL,
    volume                  REAL NOT NULL,
    komga_series_id         TEXT,
    komga_book_id           TEXT,
    mangabaka_id            INTEGER,
    mangabaka_release_id    TEXT,
    anilist_id              INTEGER,
    original_sha256         TEXT,
    normalized_sha256       TEXT,
    original_comic_info_xml TEXT,
    status                  TEXT NOT NULL, -- 'normalized', 'already_normalized', 'reverted', 'failed'
    error_message           TEXT,
    normalized_at           INTEGER NOT NULL,
    reverted_at             INTEGER
);

CREATE INDEX IF NOT EXISTS idx_norm_hist_batch ON normalization_history(batch_id);
CREATE INDEX IF NOT EXISTS idx_norm_hist_path ON normalization_history(canonical_path);
CREATE INDEX IF NOT EXISTS idx_norm_hist_series ON normalization_history(series_title);
CREATE INDEX IF NOT EXISTS idx_norm_hist_time ON normalization_history(normalized_at DESC);
