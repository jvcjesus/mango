-- Collection acquisition support: durable child unit tracking and parent grouping
ALTER TABLE downloads ADD COLUMN collection_id TEXT;
ALTER TABLE downloads ADD COLUMN collection_name TEXT;
