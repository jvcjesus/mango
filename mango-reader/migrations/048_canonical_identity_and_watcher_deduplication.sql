-- Migration 048: Canonical Identity, Watcher Deduplication, Sequential Acquisition & Integrity Repair (Milestone M11.1)

-- 1. Review Queue Acquisition Ordering & Sequential Gap Schema
ALTER TABLE acquisition_review_queue ADD COLUMN acquisition_priority INTEGER NOT NULL DEFAULT 1;
ALTER TABLE acquisition_review_queue ADD COLUMN earlier_missing_gaps TEXT;
ALTER TABLE acquisition_review_queue ADD COLUMN ordering_explanation TEXT;
ALTER TABLE acquisition_review_queue ADD COLUMN is_safe_to_approve INTEGER NOT NULL DEFAULT 1;

-- 2. Availability Watcher Canonical Series Identification
ALTER TABLE release_availability_watchers ADD COLUMN canonical_series_id TEXT;

-- Default canonical_series_id to series_id
UPDATE release_availability_watchers SET canonical_series_id = series_id WHERE canonical_series_id IS NULL;

-- 3. Canonical Manga Linkage for Haikyu!! & catalog/library mirrors
-- Formally link tracked list entry 1172 to library manga 1092
UPDATE manga SET catalog_manga_id = 1092 WHERE id = 1172 AND mangabaka_id = 8718;

-- Update canonical_series_id across all watchers referencing catalog/source mirrors
UPDATE release_availability_watchers
SET canonical_series_id = 'manga:' || (
    SELECT COALESCE(
        (SELECT id FROM manga m2 WHERE m2.id = release_availability_watchers.manga_id AND m2.in_library = 1),
        (SELECT catalog_manga_id FROM manga m2 WHERE m2.id = release_availability_watchers.manga_id AND m2.catalog_manga_id IS NOT NULL),
        (SELECT source_manga_id FROM manga m2 WHERE m2.id = release_availability_watchers.manga_id AND m2.source_manga_id IS NOT NULL),
        release_availability_watchers.manga_id
    )
)
WHERE manga_id IS NOT NULL;

-- 4. Watcher Deduplication: merge ownership detection from any sibling row into the canonical watcher row
UPDATE release_availability_watchers
SET is_owned = 1, state = 'OWNERSHIP_DETECTED', next_check_at = 0
WHERE id IN (
    SELECT MIN(w1.id)
    FROM release_availability_watchers w1
    JOIN release_availability_watchers w2 
      ON w1.series_title = w2.series_title 
     AND ABS(w1.volume_number - w2.volume_number) < 0.001
    WHERE w2.is_owned = 1
    GROUP BY w1.series_title, CAST(ROUND(w1.volume_number * 1000) AS INTEGER)
);

-- Copy best candidate from duplicate rows if min row has no top_candidate_id
UPDATE release_availability_watchers
SET top_candidate_id = (
    SELECT w2.top_candidate_id
    FROM release_availability_watchers w2
    WHERE w2.series_title = release_availability_watchers.series_title
      AND ABS(w2.volume_number - release_availability_watchers.volume_number) < 0.001
      AND w2.top_candidate_id IS NOT NULL
    LIMIT 1
),
top_candidate_provider = (
    SELECT w2.top_candidate_provider
    FROM release_availability_watchers w2
    WHERE w2.series_title = release_availability_watchers.series_title
      AND ABS(w2.volume_number - release_availability_watchers.volume_number) < 0.001
      AND w2.top_candidate_id IS NOT NULL
    LIMIT 1
),
top_candidate_title = (
    SELECT w2.top_candidate_title
    FROM release_availability_watchers w2
    WHERE w2.series_title = release_availability_watchers.series_title
      AND ABS(w2.volume_number - release_availability_watchers.volume_number) < 0.001
      AND w2.top_candidate_id IS NOT NULL
    LIMIT 1
),
top_candidate_fingerprint = (
    SELECT w2.top_candidate_fingerprint
    FROM release_availability_watchers w2
    WHERE w2.series_title = release_availability_watchers.series_title
      AND ABS(w2.volume_number - release_availability_watchers.volume_number) < 0.001
      AND w2.top_candidate_id IS NOT NULL
    LIMIT 1
),
candidate_count = (
    SELECT MAX(w2.candidate_count)
    FROM release_availability_watchers w2
    WHERE w2.series_title = release_availability_watchers.series_title
      AND ABS(w2.volume_number - release_availability_watchers.volume_number) < 0.001
)
WHERE id IN (
    SELECT MIN(w.id)
    FROM release_availability_watchers w
    GROUP BY w.series_title, CAST(ROUND(w.volume_number * 1000) AS INTEGER)
    HAVING COUNT(*) > 1
);

-- Delete redundant duplicate watcher rows FIRST
DELETE FROM release_availability_watchers
WHERE id NOT IN (
    SELECT MIN(w.id)
    FROM release_availability_watchers w
    GROUP BY w.series_title, CAST(ROUND(w.volume_number * 1000) AS INTEGER)
);

-- Point remaining canonical watcher rows for Haikyu!! to manga:1092
UPDATE release_availability_watchers
SET series_id = 'manga:1092', manga_id = 1092, canonical_series_id = 'manga:1092'
WHERE series_title = 'Haikyu!!';

-- Enforce uniqueness on (canonical_series_id, volume_number, market, edition)
CREATE UNIQUE INDEX IF NOT EXISTS idx_watchers_canonical_vol
ON release_availability_watchers(canonical_series_id, volume_number, market, edition);

-- 5. Series Reconciliation Deduplication for Haikyu!!
DELETE FROM series_reconciliations WHERE series_id IN ('manga:1066', 'manga:1172');

UPDATE series_reconciliations
SET owned_volumes_count = 4,
    owned_volumes_json = '[1.0,2.0,3.0,4.0]',
    missing_volumes_count = 41,
    missing_volumes_json = '[5.0,6.0,7.0,8.0,9.0,10.0,11.0,12.0,13.0,14.0,15.0,16.0,17.0,18.0,19.0,20.0,21.0,22.0,23.0,24.0,25.0,26.0,27.0,28.0,29.0,30.0,31.0,32.0,33.0,34.0,35.0,36.0,37.0,38.0,39.0,40.0,41.0,42.0,43.0,44.0,45.0]',
    status = 'missing_volume',
    last_error = NULL
WHERE series_id = 'manga:1092';

-- 6. Clean up obsolete out-of-order review queue proposals for Haikyu!!
DELETE FROM acquisition_review_queue WHERE series_title = 'Haikyu!!';
