-- Migration 031: Notification Outbox & Delivery State
-- Transport-independent notification queuing, claiming, and retry tracking.

CREATE TABLE notification_outbox (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id INTEGER NOT NULL REFERENCES reconciliation_events(id) ON DELETE CASCADE,
    event_type TEXT NOT NULL,
    transport TEXT NOT NULL DEFAULT 'ntfy',
    destination TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending', -- pending, sending, delivered, retry_wait, permanently_failed, suppressed
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at INTEGER NOT NULL,
    last_attempt_at INTEGER NULL,
    delivered_at INTEGER NULL,
    last_error TEXT NULL,
    created_at INTEGER NOT NULL
);

CREATE INDEX idx_notification_outbox_status_next ON notification_outbox(status, next_attempt_at);
CREATE UNIQUE INDEX idx_notification_outbox_event_transport ON notification_outbox(event_id, transport);
