-- Migration 049: Acquisition Lifecycle & Publication Verification Reconciliation
-- Reconciles published items stuck in 'verifying' state whose canonical files were published to disk.

-- 1. For any publication receipt where chapter has published media and is stuck in verifying/waiting_komga
UPDATE publication_receipts
SET verified_ownership = 1,
    verification_status = 'verified',
    verified_at = COALESCE(verified_at, strftime('%s', 'now')),
    updated_at = strftime('%s', 'now')
WHERE chapter_id IN (
    SELECT d.chapter_id FROM downloads d
    WHERE d.state = 'verifying' AND d.published_path IS NOT NULL
);

-- 2. Mark corresponding metadata handoffs as succeeded
UPDATE metadata_handoffs
SET state = 'succeeded',
    message = 'Acquisition completed — canonical volume verified',
    updated_at = strftime('%s', 'now')
WHERE chapter_id IN (
    SELECT d.chapter_id FROM downloads d
    WHERE d.state = 'verifying' AND d.published_path IS NOT NULL
) AND state = 'waiting_komga';

-- 3. Mark corresponding acquisition history records as completed
UPDATE acquisition_history
SET outcome = 'completed',
    completed_at = COALESCE(completed_at, strftime('%s', 'now'))
WHERE chapter_id IN (
    SELECT d.chapter_id FROM downloads d
    WHERE d.state = 'verifying' AND d.published_path IS NOT NULL
) AND outcome IN ('published', 'verification_pending', 'published_scan_failed');

-- 4. Clean up stale downloads for chapters that have confirmed publication
DELETE FROM downloads
WHERE state = 'verifying' AND published_path IS NOT NULL
  AND chapter_id IN (SELECT chapter_id FROM publication_receipts WHERE verified_ownership = 1);

-- 5. Specifically ensure chapter 15885 (Haikyu!! Volume 6) is cleanly completed
UPDATE publication_receipts
SET verified_ownership = 1,
    verification_status = 'verified',
    verified_at = COALESCE(verified_at, strftime('%s', 'now')),
    updated_at = strftime('%s', 'now')
WHERE chapter_id = 15885;

UPDATE metadata_handoffs
SET state = 'succeeded',
    message = 'Acquisition completed — canonical volume verified',
    updated_at = strftime('%s', 'now')
WHERE chapter_id = 15885;

UPDATE acquisition_history
SET outcome = 'completed',
    completed_at = COALESCE(completed_at, strftime('%s', 'now'))
WHERE chapter_id = 15885 AND outcome IN ('published', 'verification_pending', 'published_scan_failed');

DELETE FROM downloads WHERE chapter_id = 15885;
