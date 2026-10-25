-- Persist metadata provenance on completed/failed acquisitions
ALTER TABLE acquisition_history ADD COLUMN mangabaka_id INTEGER;
ALTER TABLE acquisition_history ADD COLUMN mangabaka_release_id TEXT;
ALTER TABLE acquisition_history ADD COLUMN anilist_id INTEGER;
ALTER TABLE acquisition_history ADD COLUMN requested_unit TEXT;
ALTER TABLE acquisition_history ADD COLUMN metadata_status TEXT;
