-- Tracked lists title metadata, aliases, and granular covers.
-- Additive columns to support English/romanized preference, alias search, and cover precedence.

ALTER TABLE tracker_list_entries ADD COLUMN english_title TEXT;
ALTER TABLE tracker_list_entries ADD COLUMN romaji_title TEXT;
ALTER TABLE tracker_list_entries ADD COLUMN native_title TEXT;
ALTER TABLE tracker_list_entries ADD COLUMN aliases TEXT;
ALTER TABLE tracker_list_entries ADD COLUMN extra_large_cover_url TEXT;
ALTER TABLE tracker_list_entries ADD COLUMN large_cover_url TEXT;
ALTER TABLE tracker_list_entries ADD COLUMN medium_cover_url TEXT;
