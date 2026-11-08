-- Migration 045: Canonical publication verification and reconciliation tracking

ALTER TABLE publication_receipts ADD COLUMN komga_book_id TEXT;
ALTER TABLE publication_receipts ADD COLUMN komga_title TEXT;
ALTER TABLE publication_receipts ADD COLUMN verification_status TEXT NOT NULL DEFAULT 'unverified';
ALTER TABLE publication_receipts ADD COLUMN verification_details TEXT;
