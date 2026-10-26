-- Migration 023: Transfer lifecycle, stall detection, and recovery tracking
ALTER TABLE downloads ADD COLUMN bytes_downloaded INTEGER DEFAULT 0;
ALTER TABLE downloads ADD COLUMN total_bytes INTEGER;
ALTER TABLE downloads ADD COLUMN last_progress_at INTEGER;
ALTER TABLE downloads ADD COLUMN is_stalled INTEGER DEFAULT 0;
