-- Migration 038: Reconciliation Integrity & Event Deduplication
--
-- 1. Adds resolved_at to reconciliation_events to track active vs historical events.
-- 2. Deduplicates existing series-level events by marking older duplicates as resolved.
-- 3. Enforces uniqueness on active series-level events via partial index.

ALTER TABLE reconciliation_events ADD COLUMN resolved_at INTEGER NULL;

-- Mark older duplicate series-level events as resolved/superseded, keeping only the latest event active.
UPDATE reconciliation_events
SET resolved_at = created_at
WHERE volume_number IS NULL
  AND id NOT IN (
      SELECT MAX(id)
      FROM reconciliation_events
      WHERE volume_number IS NULL
      GROUP BY series_id, event_type
  );

-- Enforce that for any series_id and event_type, there can only be ONE active series-level event.
CREATE UNIQUE INDEX IF NOT EXISTS uq_reconciliation_events_series_active
ON reconciliation_events(series_id, event_type)
WHERE volume_number IS NULL AND resolved_at IS NULL;
