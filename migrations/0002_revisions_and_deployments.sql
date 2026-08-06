CREATE TABLE revisions (
    id TEXT PRIMARY KEY,
    application_id TEXT NOT NULL
        REFERENCES applications(id) ON DELETE CASCADE,
    commit_sha TEXT NOT NULL,
    source_reference TEXT,
    discovered_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(application_id, commit_sha),
    UNIQUE(id, application_id)
);

CREATE TABLE deployments (
    id TEXT PRIMARY KEY,
    application_id TEXT NOT NULL
        REFERENCES applications(id) ON DELETE CASCADE,
    revision_id TEXT NOT NULL,
    status TEXT NOT NULL
        CHECK (status IN (
            'pending',
            'preparing_source',
            'building',
            'starting',
            'verifying_internal',
            'switching_traffic',
            'verifying_external',
            'succeeded',
            'failed',
            'rolling_back',
            'rolled_back'
        )),
    requested_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    started_at TEXT,
    finished_at TEXT,
    failure_code TEXT,
    failure_stage TEXT,
    failure_message TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (revision_id, application_id)
        REFERENCES revisions(id, application_id)
);

CREATE UNIQUE INDEX one_active_deployment_per_application
ON deployments(application_id)
WHERE status IN (
    'pending',
    'preparing_source',
    'building',
    'starting',
    'verifying_internal',
    'switching_traffic',
    'verifying_external',
    'rolling_back'
);
