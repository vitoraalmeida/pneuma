CREATE TABLE applications (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    desired_runtime_state TEXT NOT NULL DEFAULT 'stopped'
        CHECK (desired_runtime_state IN ('running', 'stopped')),
    spec_version INTEGER NOT NULL DEFAULT 1
        CHECK (spec_version > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE application_sources (
    application_id TEXT PRIMARY KEY
        REFERENCES applications(id) ON DELETE CASCADE,
    repository_location TEXT NOT NULL,
    repository_kind TEXT NOT NULL
        CHECK (repository_kind IN ('local', 'remote')),
    default_branch TEXT NOT NULL,
    manifest_path TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE application_build_specs (
    application_id TEXT PRIMARY KEY
        REFERENCES applications(id) ON DELETE CASCADE,
    containerfile_path TEXT NOT NULL,
    context_path TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE application_runtime_specs (
    application_id TEXT PRIMARY KEY
        REFERENCES applications(id) ON DELETE CASCADE,
    container_port INTEGER NOT NULL
        CHECK (container_port BETWEEN 1 AND 65535),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE health_check_specs (
    application_id TEXT PRIMARY KEY
        REFERENCES applications(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    expected_status INTEGER NOT NULL
        CHECK (expected_status BETWEEN 100 AND 599),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE exposures (
    application_id TEXT PRIMARY KEY
        REFERENCES applications(id) ON DELETE CASCADE,
    desired_visibility TEXT NOT NULL
        CHECK (desired_visibility IN ('internal', 'public')),
    domain TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (desired_visibility = 'internal' OR domain IS NOT NULL)
);
