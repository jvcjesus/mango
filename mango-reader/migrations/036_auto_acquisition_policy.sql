-- Migration 036: Guarded Auto-Acquisition Policies (M6.8)
--
-- Adds title-level opt-in consent and activation watermark to manga,
-- and structured auto-acquisition decision auditing to release_discovery_targets.
-- All existing titles and settings strictly default to disabled (OFF).

-- 1. Title opt-in consent and activation watermark
ALTER TABLE manga ADD COLUMN auto_acquire INTEGER NOT NULL DEFAULT 0;
ALTER TABLE manga ADD COLUMN auto_acquire_activated_at INTEGER;

-- 2. Audit fields on release_discovery_targets
ALTER TABLE release_discovery_targets ADD COLUMN auto_decision_code TEXT;
ALTER TABLE release_discovery_targets ADD COLUMN auto_decision_reason TEXT;
ALTER TABLE release_discovery_targets ADD COLUMN auto_decision_at INTEGER;

-- 3. Index for querying release targets by decision status
CREATE INDEX IF NOT EXISTS idx_release_discovery_auto_decision ON release_discovery_targets(auto_decision_code);
