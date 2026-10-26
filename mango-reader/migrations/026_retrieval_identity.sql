-- Existing candidate snapshots preserve plan identity, but direct/collection downloads
-- have no JSON provenance and external_file_id is job-local. Preserve edition intent,
-- edition-local unit, original release and exact path/size across remote recreation.
ALTER TABLE downloads ADD COLUMN retrieval_identity_json TEXT;
ALTER TABLE acquisition_history ADD COLUMN retrieval_identity_json TEXT;
