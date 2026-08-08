CREATE TABLE releases (
    id TEXT PRIMARY KEY,
    application_id TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    image_repository TEXT NOT NULL,
    image_digest TEXT NOT NULL,
    source_revision TEXT,
    created_at TEXT NOT NULL,
    UNIQUE(application_id, image_digest)
);

CREATE INDEX releases_application ON releases(application_id);
