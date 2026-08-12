ALTER TABLE deployments ADD COLUMN source_revision TEXT;

UPDATE deployments
SET source_revision = (
    SELECT releases.source_revision
    FROM releases
    WHERE releases.id = deployments.release_id
)
WHERE source_revision IS NULL;
