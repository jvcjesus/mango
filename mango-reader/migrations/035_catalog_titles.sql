-- Migration 035: First-Class Catalog Titles & Optional Reading Sources
-- Adds explicit catalog/source relationship and MangaBaka work identity directly on manga.

ALTER TABLE manga ADD COLUMN mangabaka_id INTEGER NULL;
ALTER TABLE manga ADD COLUMN source_manga_id INTEGER NULL REFERENCES manga(id) ON DELETE SET NULL;
ALTER TABLE manga ADD COLUMN catalog_manga_id INTEGER NULL REFERENCES manga(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS idx_manga_mangabaka_id ON manga(mangabaka_id);
CREATE INDEX IF NOT EXISTS idx_manga_source_manga_id ON manga(source_manga_id);
CREATE INDEX IF NOT EXISTS idx_manga_catalog_manga_id ON manga(catalog_manga_id);
