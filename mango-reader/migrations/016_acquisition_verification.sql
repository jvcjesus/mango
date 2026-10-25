-- Durable publication tracking and Komga ownership verification metadata
ALTER TABLE downloads ADD COLUMN published_path TEXT;
ALTER TABLE downloads ADD COLUMN published_at INTEGER;
ALTER TABLE downloads ADD COLUMN target_library_id TEXT;
