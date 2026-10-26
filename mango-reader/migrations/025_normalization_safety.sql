-- Freeze server-generated dry runs; never trust client-supplied media paths/metadata.
CREATE TABLE normalization_previews (
    preview_id TEXT NOT NULL,
    book_id TEXT NOT NULL,
    library_id TEXT NOT NULL,
    library_root TEXT NOT NULL,
    item_json TEXT NOT NULL,
    proposed_xml TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (preview_id, book_id)
);
ALTER TABLE normalization_history ADD COLUMN backup_path TEXT;
ALTER TABLE normalization_history ADD COLUMN staging_path TEXT;
ALTER TABLE normalization_history ADD COLUMN refresh_pending INTEGER NOT NULL DEFAULT 0;
