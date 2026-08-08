INSERT INTO releases (id, application_id, image_repository, image_digest, source_revision, created_at)
SELECT
    lower(hex(randomblob(16))),
    rev.application_id,
    'localhost/pneuma/migrated',
    rev.commit_sha,
    rev.commit_sha,
    rev.discovered_at
FROM revisions rev
WHERE NOT EXISTS (
    SELECT 1 FROM releases r
    WHERE r.application_id = rev.application_id
      AND r.source_revision = rev.commit_sha
);

CREATE TABLE deployments_new (
    id TEXT PRIMARY KEY,
    application_id TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    release_id TEXT NOT NULL REFERENCES releases(id),
    type TEXT NOT NULL DEFAULT 'deploy' CHECK (type IN ('deploy', 'rollback')),
    status TEXT NOT NULL CHECK (status IN (
        'pending', 'starting', 'verifying', 'activating', 'succeeded', 'failed'
    )),
    requested_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    started_at TEXT,
    finished_at TEXT,
    failure_code TEXT,
    failure_stage TEXT,
    failure_message TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

INSERT INTO deployments_new (
    id, application_id, release_id, type, status,
    requested_at, started_at, finished_at,
    failure_code, failure_stage, failure_message,
    created_at, updated_at
)
SELECT
    d.id,
    d.application_id,
    r.id,
    'deploy',
    CASE d.status
        WHEN 'preparing_source' THEN 'starting'
        WHEN 'building' THEN 'starting'
        WHEN 'switching_traffic' THEN 'activating'
        WHEN 'verifying_external' THEN 'activating'
        WHEN 'rolling_back' THEN 'starting'
        WHEN 'rolled_back' THEN 'failed'
        ELSE d.status
    END,
    d.requested_at,
    d.started_at,
    d.finished_at,
    d.failure_code,
    d.failure_stage,
    d.failure_message,
    d.created_at,
    d.updated_at
FROM deployments d
JOIN revisions rev ON rev.id = d.revision_id
JOIN releases r ON r.application_id = d.application_id
    AND r.source_revision = rev.commit_sha;

DROP INDEX IF EXISTS one_active_deployment_per_application;
DROP INDEX IF EXISTS deployment_identity;
DROP TABLE deployments;

ALTER TABLE deployments_new RENAME TO deployments;

CREATE UNIQUE INDEX one_active_deployment_per_application
    ON deployments(application_id)
    WHERE status IN (
        'pending', 'starting', 'verifying', 'activating'
    );

CREATE TABLE runtime_instances_new (
    id TEXT PRIMARY KEY,
    application_id TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    deployment_id TEXT NOT NULL REFERENCES deployments(id),
    external_runtime_id TEXT NOT NULL UNIQUE,
    state TEXT NOT NULL CHECK (state IN (
        'starting', 'running', 'stopped', 'failed', 'removed'
    )),
    host_address TEXT NOT NULL CHECK (host_address = '127.0.0.1'),
    host_port INTEGER NOT NULL CHECK (host_port BETWEEN 1 AND 65535),
    container_port INTEGER NOT NULL CHECK (container_port BETWEEN 1 AND 65535),
    last_observed_state TEXT NOT NULL CHECK (last_observed_state IN (
        'missing', 'created', 'starting', 'running',
        'stopping', 'stopped', 'failed', 'unknown'
    )),
    last_observed_at TEXT NOT NULL,
    exit_code INTEGER,
    observation_reason TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    removed_at TEXT
);

INSERT INTO runtime_instances_new (
    id, application_id, deployment_id, external_runtime_id, state,
    host_address, host_port, container_port,
    last_observed_state, last_observed_at,
    exit_code, observation_reason,
    created_at, updated_at, removed_at
)
SELECT
    id, application_id, deployment_id, external_runtime_id,
    CASE role
        WHEN 'current' THEN 'running'
        WHEN 'previous' THEN 'stopped'
        WHEN 'candidate' THEN 'starting'
    END,
    host_address, host_port, container_port,
    last_observed_state, last_observed_at,
    exit_code, observation_reason,
    created_at, updated_at, removed_at
FROM runtime_instances;

DROP INDEX IF EXISTS one_current_runtime_per_application;
DROP INDEX IF EXISTS active_runtime_endpoint;
DROP TABLE runtime_instances;

ALTER TABLE runtime_instances_new RENAME TO runtime_instances;

CREATE UNIQUE INDEX one_active_runtime_per_application
    ON runtime_instances(application_id)
    WHERE state = 'running' AND removed_at IS NULL;

CREATE UNIQUE INDEX active_runtime_endpoint
    ON runtime_instances(host_address, host_port)
    WHERE removed_at IS NULL;

ALTER TABLE applications ADD COLUMN active_deployment_id TEXT REFERENCES deployments(id);

UPDATE applications SET active_deployment_id = (
    SELECT d.id FROM deployments d
    JOIN runtime_instances ri ON ri.deployment_id = d.id
    WHERE ri.application_id = applications.id
      AND ri.state = 'running'
      AND ri.removed_at IS NULL
    LIMIT 1
);
