CREATE UNIQUE INDEX deployment_identity
ON deployments(id, application_id, revision_id);

CREATE TABLE runtime_instances (
    id TEXT PRIMARY KEY,
    application_id TEXT NOT NULL
        REFERENCES applications(id) ON DELETE CASCADE,
    revision_id TEXT NOT NULL,
    deployment_id TEXT NOT NULL,
    external_runtime_id TEXT NOT NULL UNIQUE,
    role TEXT NOT NULL
        CHECK (role IN ('candidate', 'current', 'previous')),
    host_address TEXT NOT NULL
        CHECK (host_address = '127.0.0.1'),
    host_port INTEGER NOT NULL
        CHECK (host_port BETWEEN 1 AND 65535),
    container_port INTEGER NOT NULL
        CHECK (container_port BETWEEN 1 AND 65535),
    last_observed_state TEXT NOT NULL
        CHECK (last_observed_state IN (
            'missing',
            'created',
            'starting',
            'running',
            'stopping',
            'stopped',
            'failed',
            'unknown'
        )),
    last_observed_at TEXT NOT NULL,
    exit_code INTEGER,
    observation_reason TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    removed_at TEXT,
    FOREIGN KEY (deployment_id, application_id, revision_id)
        REFERENCES deployments(id, application_id, revision_id)
);

CREATE UNIQUE INDEX one_current_runtime_per_application
ON runtime_instances(application_id)
WHERE role = 'current' AND removed_at IS NULL;

CREATE UNIQUE INDEX active_runtime_endpoint
ON runtime_instances(host_address, host_port)
WHERE removed_at IS NULL;
