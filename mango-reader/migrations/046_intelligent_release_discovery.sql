-- Migration 046: Intelligent Release Discovery, Multi-Source Ranking & Fallback Planning

-- Add normalized metadata, confidence, and ranking explanation columns to discovered_release_candidates
ALTER TABLE discovered_release_candidates ADD COLUMN confidence TEXT NOT NULL DEFAULT 'high';
ALTER TABLE discovered_release_candidates ADD COLUMN confidence_reason TEXT NULL;
ALTER TABLE discovered_release_candidates ADD COLUMN language TEXT NOT NULL DEFAULT 'en';
ALTER TABLE discovered_release_candidates ADD COLUMN edition TEXT NULL;
ALTER TABLE discovered_release_candidates ADD COLUMN format TEXT NULL;
ALTER TABLE discovered_release_candidates ADD COLUMN resolution TEXT NULL;
ALTER TABLE discovered_release_candidates ADD COLUMN publisher TEXT NULL;
ALTER TABLE discovered_release_candidates ADD COLUMN ranking_breakdown_json TEXT NULL;

-- Durable fallback history table for failed acquisitions
CREATE TABLE acquisition_fallback_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    chapter_id INTEGER NOT NULL,
    manga_id INTEGER NOT NULL,
    series_title TEXT NOT NULL,
    volume_number REAL NULL,
    failed_candidate_id TEXT NOT NULL,
    failed_provider TEXT NOT NULL,
    failed_reason TEXT NOT NULL,
    fallback_candidate_id TEXT NOT NULL,
    fallback_provider TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'switched',
    created_at INTEGER NOT NULL
);
CREATE INDEX idx_acquisition_fallback_chapter ON acquisition_fallback_history(chapter_id);
