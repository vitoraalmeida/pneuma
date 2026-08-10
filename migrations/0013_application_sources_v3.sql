CREATE TABLE application_sources_new (
    application_id TEXT PRIMARY KEY
        REFERENCES applications(id) ON DELETE CASCADE,
    repository_url TEXT NOT NULL,
    repository_kind TEXT NOT NULL
        CHECK (repository_kind IN ('local', 'remote')),
    default_branch TEXT,
    manifest_path TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

INSERT INTO application_sources_new (
    application_id, repository_url, repository_kind,
    default_branch, manifest_path, created_at, updated_at
)
SELECT
    application_id, repository_location, repository_kind,
    default_branch, manifest_path, created_at, updated_at
FROM application_sources;

DROP TABLE application_sources;

ALTER TABLE application_sources_new RENAME TO application_sources;
