-- Migration 024: Durable magnet URI provenance for torrent recreation
ALTER TABLE downloads ADD COLUMN magnet_url TEXT;
