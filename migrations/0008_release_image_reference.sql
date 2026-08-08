ALTER TABLE releases ADD COLUMN image_reference TEXT;

UPDATE releases
SET image_reference = image_repository || ':' || image_digest
WHERE image_reference IS NULL;
