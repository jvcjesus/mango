-- Migration 037: Madokami direct acquisition provider and cross-provider atomic safety (M6.8.1)
--
-- Guarantees at the database constraint level that only one active candidate approval
-- may exist for any logical series volume across all providers, schedulers, and workers.

CREATE UNIQUE INDEX IF NOT EXISTS idx_candidate_approvals_active_unique
ON candidate_approvals(series_id, volume_number)
WHERE status IN ('queued', 'approved', 'acquiring');
