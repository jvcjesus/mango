-- Per-publication metadata delivery state. Identity mappings remain in existing provenance.
CREATE TABLE metadata_handoffs (
    chapter_id INTEGER PRIMARY KEY REFERENCES chapters(id) ON DELETE CASCADE,
    canonical_path TEXT NOT NULL,
    state TEXT NOT NULL,
    komf_url TEXT,
    komga_book_id TEXT,
    komga_series_id TEXT,
    library_id TEXT,
    mangabaka_id INTEGER,
    job_id TEXT,
    message TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
