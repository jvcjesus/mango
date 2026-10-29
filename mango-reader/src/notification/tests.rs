use super::config::*;
use super::outbox::*;
use super::types::*;
use crate::db::now;
use rusqlite::Connection;

fn test_db() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "
        CREATE TABLE state (key TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE TABLE reconciliation_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            series_id TEXT NOT NULL,
            mangabaka_id INTEGER NULL,
            mangabaka_volume_id TEXT NULL,
            series_title TEXT NOT NULL,
            edition TEXT NOT NULL DEFAULT 'standard',
            volume_number REAL NULL,
            event_type TEXT NOT NULL,
            top_candidate_id TEXT NULL,
            candidate_count INTEGER NOT NULL DEFAULT 0,
            message TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            notified_at INTEGER NULL,
            UNIQUE(series_id, volume_number, event_type)
        );
        CREATE TABLE series_reconciliations (
            series_id TEXT PRIMARY KEY,
            manga_id INTEGER NULL,
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
        CREATE TABLE notification_outbox (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            event_id INTEGER NOT NULL REFERENCES reconciliation_events(id) ON DELETE CASCADE,
            event_type TEXT NOT NULL,
            transport TEXT NOT NULL DEFAULT 'ntfy',
            destination TEXT NOT NULL,
            payload_json TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            attempts INTEGER NOT NULL DEFAULT 0,
            next_attempt_at INTEGER NOT NULL,
            last_attempt_at INTEGER NULL,
            delivered_at INTEGER NULL,
            last_error TEXT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE INDEX idx_notification_outbox_status_next ON notification_outbox(status, next_attempt_at);
        CREATE UNIQUE INDEX idx_notification_outbox_event_transport ON notification_outbox(event_id, transport);
        ",
    )
    .unwrap();
    conn
}

#[test]
fn test_config_defaults_and_round_trip() {
    let conn = test_db();
    let initial = get_ntfy_config(&conn);
    assert!(!initial.enabled);
    assert_eq!(initial.server_url, "https://ntfy.sh");
    assert!(initial.topic.is_empty());
    assert!(initial.token.is_none());
    assert!(initial.pref_new_volume);
    assert!(initial.pref_release_available);
    assert!(initial.pref_acquisition_complete);
    assert!(initial.pref_action_required);
    assert!(initial.activated_at.is_none());

    let dto = NtfyConfigDto::from(&initial);
    assert!(!dto.has_token);
    assert!(dto.token.is_none());

    // Update config with auth token
    let updated = NtfyConfig {
        enabled: true,
        server_url: "https://ntfy.example.com".to_string(),
        topic: "my_manga_alerts".to_string(),
        token: Some("secret_ntfy_token".to_string()),
        pref_new_volume: true,
        pref_release_available: false,
        pref_acquisition_complete: true,
        pref_action_required: true,
        activated_at: None,
    };
    set_ntfy_config(&conn, &updated).unwrap();

    let fetched = get_ntfy_config(&conn);
    assert!(fetched.enabled);
    assert_eq!(fetched.server_url, "https://ntfy.example.com");
    assert_eq!(fetched.topic, "my_manga_alerts");
    assert_eq!(fetched.token.as_deref(), Some("secret_ntfy_token"));
    assert!(!fetched.pref_release_available);
    assert!(fetched.activated_at.is_some());

    // Verify DTO hides token
    let dto_updated = NtfyConfigDto::from(&fetched);
    assert!(dto_updated.has_token);
    assert!(dto_updated.token.is_none());
}

#[test]
fn test_watermark_historical_flood_protection() {
    let conn = test_db();

    // 1. Pre-existing domain event created at t=1000 before ntfy was enabled
    conn.execute(
        "INSERT INTO reconciliation_events (
            id, series_id, series_title, volume_number, event_type, message, created_at
        ) VALUES (1, 'manga:10', 'Vinland Saga', 5.0, 'NEW_VOLUME', 'Vol 5 released', 1000)",
        [],
    )
    .unwrap();

    // 2. Enable ntfy at t=2000
    conn.execute("INSERT INTO state (key, value) VALUES ('notification.ntfy_enabled', 'true')", []).unwrap();
    conn.execute("INSERT INTO state (key, value) VALUES ('notification.ntfy_topic', 'test-topic')", []).unwrap();
    conn.execute("INSERT INTO state (key, value) VALUES ('notification.activated_at', '2000')", []).unwrap();

    // 3. Attempt to enqueue pre-existing event (created_at = 1000 <= activated_at = 2000)
    let outbox_id = enqueue_from_event(&conn, 1, "manga:10", "Vinland Saga", Some(5.0), "NEW_VOLUME", "Vol 5 released").unwrap();
    assert!(outbox_id.is_some());

    // Verify that historical event is marked 'suppressed' and will never be sent
    let status: String = conn.query_row("SELECT status FROM notification_outbox WHERE event_id = 1", [], |r| r.get(0)).unwrap();
    assert_eq!(status, "suppressed");

    // 4. New event created at t=3000 (> activated_at = 2000)
    conn.execute(
        "INSERT INTO reconciliation_events (
            id, series_id, series_title, volume_number, event_type, message, created_at
        ) VALUES (2, 'manga:10', 'Vinland Saga', 6.0, 'NEW_VOLUME', 'Vol 6 released', 3000)",
        [],
    )
    .unwrap();

    let outbox_id2 = enqueue_from_event(&conn, 2, "manga:10", "Vinland Saga", Some(6.0), "NEW_VOLUME", "Vol 6 released").unwrap();
    assert!(outbox_id2.is_some());

    // Verify that new event has status 'pending'
    let status2: String = conn.query_row("SELECT status FROM notification_outbox WHERE event_id = 2", [], |r| r.get(0)).unwrap();
    assert_eq!(status2, "pending");
}

#[test]
fn test_preference_filtering() {
    let conn = test_db();
    let now_ts = now();

    conn.execute("INSERT INTO state (key, value) VALUES ('notification.ntfy_enabled', 'true')", []).unwrap();
    conn.execute("INSERT INTO state (key, value) VALUES ('notification.ntfy_topic', 'test-topic')", []).unwrap();
    conn.execute("INSERT INTO state (key, value) VALUES ('notification.pref_release_available', 'false')", []).unwrap();
    conn.execute(&format!("INSERT INTO state (key, value) VALUES ('notification.activated_at', '{}')", now_ts - 100), []).unwrap();

    // Event 1: RELEASE_AVAILABLE (disabled in preferences)
    conn.execute(
        "INSERT INTO reconciliation_events (
            id, series_id, series_title, volume_number, event_type, message, created_at
        ) VALUES (1, 'manga:10', 'Berserk', 12.0, 'RELEASE_AVAILABLE', 'Found release', ?1)",
        [now_ts],
    )
    .unwrap();

    enqueue_from_event(&conn, 1, "manga:10", "Berserk", Some(12.0), "RELEASE_AVAILABLE", "Found release").unwrap();
    let status: String = conn.query_row("SELECT status FROM notification_outbox WHERE event_id = 1", [], |r| r.get(0)).unwrap();
    assert_eq!(status, "suppressed");

    // Event 2: ACQUISITION_COMPLETE (enabled by default)
    conn.execute(
        "INSERT INTO reconciliation_events (
            id, series_id, series_title, volume_number, event_type, message, created_at
        ) VALUES (2, 'manga:10', 'Berserk', 12.0, 'ACQUISITION_COMPLETE', 'Published Vol 12', ?1)",
        [now_ts],
    )
    .unwrap();

    enqueue_from_event(&conn, 2, "manga:10", "Berserk", Some(12.0), "ACQUISITION_COMPLETE", "Published Vol 12").unwrap();
    let status2: String = conn.query_row("SELECT status FROM notification_outbox WHERE event_id = 2", [], |r| r.get(0)).unwrap();
    assert_eq!(status2, "pending");
}

#[test]
fn test_payload_formatting_and_action_links() {
    let cfg = NtfyConfig {
        enabled: true,
        server_url: "https://ntfy.sh".to_string(),
        topic: "mango-test".to_string(),
        token: None,
        pref_new_volume: true,
        pref_release_available: true,
        pref_acquisition_complete: true,
        pref_action_required: true,
        activated_at: Some(100),
    };

    // 1. NEW_VOLUME
    let p1 = build_payload(&cfg, NotificationEventType::NewVolume, "Vinland Saga", Some(17.0), "New volume 17", Some(42));
    assert_eq!(p1.title, "📚 Vinland Saga Vol. 17");
    assert_eq!(p1.tags, vec!["books"]);
    assert_eq!(p1.priority, 3);
    assert_eq!(p1.click, Some("/title/42".to_string()));

    // 2. RELEASE_AVAILABLE with fractional volume
    let p2 = build_payload(&cfg, NotificationEventType::ReleaseAvailable, "Grand Blue", Some(12.5), "Found 3 candidates", Some(99));
    assert_eq!(p2.title, "🔎 Grand Blue Vol. 12.5");
    assert_eq!(p2.tags, vec!["mag"]);
    assert_eq!(p2.priority, 3);
    assert_eq!(p2.click, Some("/title/99".to_string()));

    // 3. ACQUISITION_COMPLETE
    let p3 = build_payload(&cfg, NotificationEventType::AcquisitionComplete, "Berserk", Some(1.0), "Published Berserk Vol. 1", None);
    assert_eq!(p3.title, "✅ Berserk Vol. 1");
    assert_eq!(p3.tags, vec!["white_check_mark"]);
    assert_eq!(p3.priority, 3);
    assert_eq!(p3.click, None);

    // 4. ACTION_REQUIRED (no volume number)
    let p4 = build_payload(&cfg, NotificationEventType::ActionRequired, "Ambiguous Manga", None, "Multiple MangaBaka matches found", Some(10));
    assert_eq!(p4.title, "⚠️ Ambiguous Manga needs attention");
    assert_eq!(p4.tags, vec!["warning"]);
    assert_eq!(p4.priority, 4);
    assert_eq!(p4.click, Some("/title/10".to_string()));
}

#[test]
fn test_idempotent_enqueueing() {
    let conn = test_db();
    let now_ts = now();

    conn.execute("INSERT INTO state (key, value) VALUES ('notification.ntfy_enabled', 'true')", []).unwrap();
    conn.execute("INSERT INTO state (key, value) VALUES ('notification.ntfy_topic', 'test-topic')", []).unwrap();
    conn.execute(&format!("INSERT INTO state (key, value) VALUES ('notification.activated_at', '{}')", now_ts - 50), []).unwrap();

    conn.execute(
        "INSERT INTO reconciliation_events (
            id, series_id, series_title, volume_number, event_type, message, created_at
        ) VALUES (1, 'manga:1', 'One Piece', 105.0, 'NEW_VOLUME', 'Vol 105', ?1)",
        [now_ts],
    )
    .unwrap();

    // First enqueue
    let id1 = enqueue_from_event(&conn, 1, "manga:1", "One Piece", Some(105.0), "NEW_VOLUME", "Vol 105").unwrap();
    assert!(id1.is_some());

    // Second enqueue (must be idempotent and NOT create a second row)
    let id2 = enqueue_from_event(&conn, 1, "manga:1", "One Piece", Some(105.0), "NEW_VOLUME", "Vol 105").unwrap();
    assert_eq!(id1, id2);

    let count: i64 = conn.query_row("SELECT count(*) FROM notification_outbox", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 1);
}

#[test]
fn test_batch_claiming_and_stale_recovery() {
    let conn = test_db();
    let now_ts = now();

    conn.execute_batch(
        "INSERT INTO reconciliation_events (id, series_id, series_title, event_type, message, created_at)
         VALUES
            (10, 's1', 'Series 1', 'NEW_VOLUME', 'msg', 100),
            (11, 's2', 'Series 2', 'NEW_VOLUME', 'msg', 100),
            (12, 's3', 'Series 3', 'NEW_VOLUME', 'msg', 100);",
    )
    .unwrap();

    // Insert 3 items: 2 pending, 1 stale sending (>5 minutes ago)
    conn.execute(
        "INSERT INTO notification_outbox (
            id, event_id, event_type, transport, destination, payload_json, status, attempts, next_attempt_at, last_attempt_at, created_at
        ) VALUES
            (1, 10, 'NEW_VOLUME', 'ntfy', 'topic', '{}', 'pending', 0, ?1, NULL, ?1),
            (2, 11, 'NEW_VOLUME', 'ntfy', 'topic', '{}', 'pending', 0, ?1, NULL, ?1),
            (3, 12, 'NEW_VOLUME', 'ntfy', 'topic', '{}', 'sending', 1, ?1, ?2, ?1)",
        [now_ts, now_ts - 400],
    )
    .unwrap();

    // Claim batch of 2
    let claimed = claim_pending_batch(&conn, 2).unwrap();
    assert_eq!(claimed.len(), 2);

    // Verify row 3 was recovered from stale sending back to retry_wait
    let status_3: String = conn.query_row("SELECT status FROM notification_outbox WHERE id = 3", [], |r| r.get(0)).unwrap();
    assert_eq!(status_3, "retry_wait");
}

#[test]
fn test_retry_backoff_and_permanent_failure() {
    let conn = test_db();
    let now_ts = now();

    conn.execute(
        "INSERT INTO reconciliation_events (id, series_id, series_title, event_type, message, created_at)
         VALUES (10, 's1', 'Series 1', 'NEW_VOLUME', 'msg', 100)",
        [],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO notification_outbox (
            id, event_id, event_type, transport, destination, payload_json, status, attempts, next_attempt_at, created_at
        ) VALUES (1, 10, 'NEW_VOLUME', 'ntfy', 'topic', '{}', 'sending', 1, ?1, ?1)",
        [now_ts],
    )
    .unwrap();

    // Attempt 1 fails transiently
    mark_retry(&conn, 1, 1, "HTTP 429 Rate limited").unwrap();
    let (status, next_at, error): (String, i64, Option<String>) = conn
        .query_row(
            "SELECT status, next_attempt_at, last_error FROM notification_outbox WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();

    assert_eq!(status, "retry_wait");
    assert!(next_at >= now_ts + 29); // Backoff is 30s
    assert_eq!(error.as_deref(), Some("HTTP 429 Rate limited"));

    // Attempt 5 fails -> transitions to permanently_failed
    mark_retry(&conn, 1, 5, "HTTP 500 Server error").unwrap();
    let (status_5, error_5): (String, Option<String>) = conn
        .query_row(
            "SELECT status, last_error FROM notification_outbox WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();

    assert_eq!(status_5, "permanently_failed");
    assert!(error_5.unwrap().contains("Max retry attempts reached"));
}

#[test]
fn test_delivery_success_updates_both_tables() {
    let conn = test_db();
    let now_ts = now();

    conn.execute(
        "INSERT INTO reconciliation_events (
            id, series_id, series_title, volume_number, event_type, message, created_at, notified_at
        ) VALUES (10, 'manga:1', 'Chainsaw Man', 14.0, 'NEW_VOLUME', 'Vol 14', ?1, NULL)",
        [now_ts],
    )
    .unwrap();

    conn.execute(
        "INSERT INTO notification_outbox (
            id, event_id, event_type, transport, destination, payload_json, status, attempts, next_attempt_at, created_at
        ) VALUES (1, 10, 'NEW_VOLUME', 'ntfy', 'topic', '{}', 'sending', 1, ?1, ?1)",
        [now_ts],
    )
    .unwrap();

    mark_delivered(&conn, 1, 10).unwrap();

    let outbox_status: String = conn.query_row("SELECT status FROM notification_outbox WHERE id = 1", [], |r| r.get(0)).unwrap();
    let outbox_delivered: Option<i64> = conn.query_row("SELECT delivered_at FROM notification_outbox WHERE id = 1", [], |r| r.get(0)).unwrap();
    let event_notified: Option<i64> = conn.query_row("SELECT notified_at FROM reconciliation_events WHERE id = 10", [], |r| r.get(0)).unwrap();

    assert_eq!(outbox_status, "delivered");
    assert!(outbox_delivered.is_some());
    assert!(event_notified.is_some());
}

#[test]
fn test_physical_media_invariant() {
    // Proves that notification payload and outbox contain strictly read-only click links
    // and no remote action parameters, executable triggers, or acquisition endpoints.
    let cfg = NtfyConfig {
        enabled: true,
        server_url: "https://ntfy.sh".to_string(),
        topic: "my_topic".to_string(),
        token: None,
        pref_new_volume: true,
        pref_release_available: true,
        pref_acquisition_complete: true,
        pref_action_required: true,
        activated_at: Some(10),
    };

    let p = build_payload(&cfg, NotificationEventType::ReleaseAvailable, "Frieren", Some(10.0), "Found candidate", Some(55));
    let json = serde_json::to_string(&p).unwrap();

    assert!(!json.contains("download"));
    assert!(!json.contains("acquire"));
    assert!(!json.contains("torbox"));
    assert_eq!(p.click, Some("/title/55".to_string()));
}
