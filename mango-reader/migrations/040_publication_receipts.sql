CREATE TABLE IF NOT EXISTS publication_receipts (
    id                  TEXT PRIMARY KEY,
    chapter_id          INTEGER NOT NULL,
    manga_id            INTEGER REFERENCES manga(id) ON DELETE SET NULL,
    series_title        TEXT NOT NULL,
    volume_number       REAL,
    chapter_number      REAL,
    is_volume           INTEGER NOT NULL DEFAULT 1,
    canonical_path      TEXT NOT NULL,
    file_size_bytes     INTEGER NOT NULL,
    file_sha256         TEXT NOT NULL,
    published_at        INTEGER NOT NULL,
    komga_notified      INTEGER NOT NULL DEFAULT 0,
    komga_library_id    TEXT,
    metadata_status     TEXT NOT NULL DEFAULT 'complete',
    metadata_error      TEXT,
    verified_ownership  INTEGER NOT NULL DEFAULT 0,
    verified_at         INTEGER,
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    UNIQUE(canonical_path)
);

CREATE INDEX IF NOT EXISTS idx_pub_receipts_chapter ON publication_receipts(chapter_id);
CREATE INDEX IF NOT EXISTS idx_pub_receipts_manga ON publication_receipts(manga_id);
CREATE INDEX IF NOT EXISTS idx_pub_receipts_path ON publication_receipts(canonical_path);
