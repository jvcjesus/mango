-- External release discovery (Nyaa) and download backend (TorBox) metadata
ALTER TABLE downloads ADD COLUMN source_type TEXT NOT NULL DEFAULT 'reader';
ALTER TABLE downloads ADD COLUMN backend_type TEXT NOT NULL DEFAULT 'internal';
ALTER TABLE downloads ADD COLUMN external_job_id TEXT;
ALTER TABLE downloads ADD COLUMN external_file_id INTEGER;
ALTER TABLE downloads ADD COLUMN torrent_hash TEXT;
ALTER TABLE downloads ADD COLUMN raw_title TEXT;
ALTER TABLE downloads ADD COLUMN uploader TEXT;
ALTER TABLE downloads ADD COLUMN release_size INTEGER;
ALTER TABLE downloads ADD COLUMN retrieved_path TEXT;

-- Durable acquisition history preserving completed/failed releases across sources
CREATE TABLE IF NOT EXISTS acquisition_history (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    chapter_id      INTEGER,
    manga_id        INTEGER NOT NULL REFERENCES manga(id) ON DELETE CASCADE,
    series          TEXT NOT NULL,
    release_name    TEXT NOT NULL,
    source_type     TEXT NOT NULL DEFAULT 'reader',
    uploader        TEXT,
    external_job_id TEXT,
    backend_type    TEXT NOT NULL DEFAULT 'internal',
    outcome         TEXT NOT NULL,
    canonical_path  TEXT,
    release_size    INTEGER,
    completed_at    INTEGER NOT NULL,
    error           TEXT
);

CREATE INDEX IF NOT EXISTS idx_acq_hist_manga ON acquisition_history(manga_id);
CREATE INDEX IF NOT EXISTS idx_acq_hist_time ON acquisition_history(completed_at DESC);
