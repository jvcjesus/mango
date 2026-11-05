-- M8.1: Unified acquisition UX, source integration, and provider intelligence
INSERT OR IGNORE INTO source_intelligence (
    source_id, name, supports_volumes, supports_chapters, supports_collections,
    requires_external_backend, status, updated_at
) VALUES 
('madokami', 'Madokami', 1, 1, 0, 0, 'healthy', CAST(strftime('%s', 'now') AS INTEGER));
