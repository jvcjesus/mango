-- Migration 044: Naming standardization audit and durable metadata correction backups

CREATE TABLE IF NOT EXISTS naming_audit_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    audit_id TEXT NOT NULL UNIQUE,
    library_id TEXT NOT NULL,
    series_id TEXT,
    total_series INTEGER NOT NULL,
    total_books INTEGER NOT NULL,
    inconsistent_titles INTEGER NOT NULL,
    inconsistent_numbers INTEGER NOT NULL,
    canonical_count INTEGER NOT NULL,
    details_json TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_naming_audit_created ON naming_audit_logs(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_naming_audit_library ON naming_audit_logs(library_id);

CREATE TABLE IF NOT EXISTS naming_correction_backups (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    backup_id TEXT NOT NULL,
    book_id TEXT NOT NULL,
    series_id TEXT NOT NULL,
    series_title TEXT NOT NULL,
    original_title TEXT,
    original_number TEXT,
    original_number_sort REAL,
    new_title TEXT NOT NULL,
    new_number TEXT NOT NULL,
    new_number_sort REAL NOT NULL,
    status TEXT NOT NULL DEFAULT 'applied',
    cbz_updated INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    rolled_back_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_naming_backups_batch ON naming_correction_backups(backup_id);
CREATE INDEX IF NOT EXISTS idx_naming_backups_book ON naming_correction_backups(book_id);
