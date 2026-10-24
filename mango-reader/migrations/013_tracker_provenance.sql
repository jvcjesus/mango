-- Tracker identity provenance, edition resolution, and dimension-aware progress.

ALTER TABLE tracker_mappings ADD COLUMN mapping_source TEXT NOT NULL DEFAULT 'manual';
ALTER TABLE tracker_mappings ADD COLUMN edition_id TEXT;
ALTER TABLE tracker_mappings ADD COLUMN confidence TEXT NOT NULL DEFAULT 'resolved';
ALTER TABLE tracker_mappings ADD COLUMN last_validated INTEGER;

ALTER TABLE sync_records ADD COLUMN dimension TEXT;
