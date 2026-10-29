-- Migration 029: Ongoing series reconciliation and event logs.
-- Stores durable reconciliation state between MangaBaka official volumes and Komga owned content.

CREATE TABLE IF NOT EXISTS series_reconciliations (
    series_id TEXT PRIMARY KEY,
    manga_id INTEGER NULL REFERENCES manga(id) ON DELETE SET NULL,
    komga_series_id TEXT NULL,
    mangabaka_id INTEGER NULL,
    collection_id TEXT NULL,
    title TEXT NOT NULL,
    edition TEXT NOT NULL DEFAULT 'standard',
    status TEXT NOT NULL,
    is_ongoing INTEGER NOT NULL DEFAULT 1,
    total_official_volumes INTEGER NULL,
    owned_volumes_count INTEGER NOT NULL DEFAULT 0,
    missing_volumes_count INTEGER NOT NULL DEFAULT 0,
    owned_volumes_json TEXT NOT NULL DEFAULT '[]',
    missing_volumes_json TEXT NOT NULL DEFAULT '[]',
    future_volumes_json TEXT NOT NULL DEFAULT '[]',
    last_reconciled_at INTEGER NOT NULL,
    last_error TEXT NULL,
    review_reason TEXT NULL,
    updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_reconciliations_manga_id ON series_reconciliations(manga_id);
CREATE INDEX IF NOT EXISTS idx_reconciliations_komga_id ON series_reconciliations(komga_series_id);
CREATE INDEX IF NOT EXISTS idx_reconciliations_status ON series_reconciliations(status);

CREATE TABLE IF NOT EXISTS reconciliation_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    series_title TEXT NOT NULL,
    mangabaka_id INTEGER NULL,
    volume_number REAL NULL,
    event_type TEXT NOT NULL,
    message TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    notified_at INTEGER NULL,
    UNIQUE(series_title, volume_number, event_type)
);

CREATE INDEX IF NOT EXISTS idx_reconciliation_events_series ON reconciliation_events(series_title);
CREATE INDEX IF NOT EXISTS idx_reconciliation_events_notified ON reconciliation_events(notified_at);
