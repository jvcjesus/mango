CREATE TABLE import_payloads (
 chapter_id INTEGER PRIMARY KEY REFERENCES chapters(id) ON DELETE CASCADE,
 local_path TEXT NOT NULL,
 provenance_json TEXT NOT NULL,
 source_hash TEXT,
 state TEXT NOT NULL CHECK(state IN ('inspected','needs_review','cancelled','cleaned')),
 inspected_at INTEGER NOT NULL
);
CREATE TABLE import_candidates (
 id INTEGER PRIMARY KEY AUTOINCREMENT,
 payload_chapter INTEGER NOT NULL REFERENCES import_payloads(chapter_id) ON DELETE CASCADE,
 inner_path TEXT NOT NULL,
 local_path TEXT NOT NULL,
 suggested_number REAL,
 state TEXT NOT NULL DEFAULT 'needs_review' CHECK(state IN ('needs_review','queued','processing','ignored')),
 cancel_requested INTEGER NOT NULL DEFAULT 0,
 assigned_series TEXT,
 assigned_kind TEXT CHECK(assigned_kind IN ('volume','chapter')),
 assigned_number REAL,
 assigned_edition TEXT,
 assigned_at INTEGER,
 child_chapter INTEGER REFERENCES chapters(id) ON DELETE SET NULL,
 UNIQUE(payload_chapter,inner_path)
);

CREATE INDEX import_payload_state ON import_payloads(state);
CREATE INDEX IF NOT EXISTS import_history_chapter ON acquisition_history(chapter_id,id);
