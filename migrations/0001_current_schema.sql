-- Pneuma current schema baseline.
--
-- This single migration replaces the entire historical migration chain.
-- Databases created before it are incompatible: an empty database initializes
-- atomically with this file, a database carrying the matching
-- `schema_migrations` ledger row reopens normally, and every other non-empty
-- schema is rejected at open time.

CREATE TABLE schema_migrations (
    migration_id TEXT PRIMARY KEY,
    applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE systems (
    id TEXT PRIMARY KEY CHECK (
        length(id) = 32 AND lower(id) = id
        AND trim(id, '0123456789abcdef') = ''
    ),
    name TEXT NOT NULL UNIQUE,
    description TEXT
);

CREATE TABLE applications (
    id TEXT PRIMARY KEY CHECK (
        length(id) = 32 AND lower(id) = id
        AND trim(id, '0123456789abcdef') = ''
    ),
    system_id TEXT NOT NULL REFERENCES systems(id),
    name TEXT NOT NULL UNIQUE,
    repository_url TEXT NOT NULL,
    default_branch TEXT,
    manifest_path TEXT NOT NULL,
    image_repository TEXT NOT NULL,
    container_port INTEGER NOT NULL
        CHECK (container_port BETWEEN 1 AND 65535),
    health_check_path TEXT NOT NULL,
    health_check_expected_status INTEGER NOT NULL
        CHECK (health_check_expected_status BETWEEN 100 AND 599),
    desired_runtime_state TEXT NOT NULL
        CHECK (desired_runtime_state IN ('running', 'stopped')),
    active_deployment_id TEXT,
    -- The active Deployment must belong to this Application; the guarded
    -- activation write additionally requires it to be Succeeded.
    FOREIGN KEY (active_deployment_id, id)
        REFERENCES deployments(id, application_id)
);

CREATE TABLE releases (
    id TEXT PRIMARY KEY CHECK (
        length(id) = 32 AND lower(id) = id
        AND trim(id, '0123456789abcdef') = ''
    ),
    application_id TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    -- Canonical digest-pinned reference; repository and digest are derived by
    -- parsing this value, never stored separately.
    image_reference TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (application_id, image_reference),
    UNIQUE (id, application_id)
);

CREATE TABLE deployments (
    id TEXT PRIMARY KEY CHECK (
        length(id) = 32 AND lower(id) = id
        AND trim(id, '0123456789abcdef') = ''
    ),
    application_id TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    release_id TEXT NOT NULL,
    type TEXT NOT NULL CHECK (type IN ('deploy', 'rollback')),
    status TEXT NOT NULL CHECK (status IN (
        'pending', 'starting', 'verifying', 'activating', 'succeeded', 'failed'
    )),
    source_revision TEXT
        CHECK (source_revision IS NULL OR (
            length(source_revision) = 40
            AND lower(source_revision) = source_revision
            AND trim(source_revision, '0123456789abcdef') = ''
        )),
    requested_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    started_at TEXT,
    finished_at TEXT,
    failure_code TEXT,
    failure_stage TEXT
        CHECK (failure_stage IS NULL OR failure_stage IN (
            'pending', 'starting', 'verifying', 'activating'
        )),
    failure_message TEXT,
    UNIQUE (id, application_id),
    FOREIGN KEY (release_id, application_id)
        REFERENCES releases(id, application_id),
    -- Complete terminal evidence matrix: non-terminal rows carry no terminal
    -- evidence, succeeded rows carry only finished_at, and failed rows carry
    -- the full failure triple.
    CHECK (
        (status IN ('pending', 'starting', 'verifying', 'activating')
            AND finished_at IS NULL
            AND failure_code IS NULL
            AND failure_stage IS NULL
            AND failure_message IS NULL)
        OR (status = 'succeeded'
            AND finished_at IS NOT NULL
            AND failure_code IS NULL
            AND failure_stage IS NULL
            AND failure_message IS NULL)
        OR (status = 'failed'
            AND finished_at IS NOT NULL
            AND failure_code IS NOT NULL
            AND failure_stage IS NOT NULL
            AND failure_message IS NOT NULL)
    )
);

CREATE UNIQUE INDEX one_in_progress_deployment_per_application
    ON deployments(application_id)
    WHERE status IN ('pending', 'starting', 'verifying', 'activating');

CREATE INDEX deployments_application_history
    ON deployments(application_id, requested_at);

CREATE TABLE runtime_instances (
    id TEXT PRIMARY KEY CHECK (
        length(id) = 32 AND lower(id) = id
        AND trim(id, '0123456789abcdef') = ''
    ),
    application_id TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    deployment_id TEXT NOT NULL,
    external_runtime_id TEXT NOT NULL UNIQUE
        CHECK (length(external_runtime_id) > 0
            AND trim(external_runtime_id, '0123456789abcdefABCDEF') = ''),
    -- The endpoint is always 127.0.0.1 and is therefore not persisted.
    state TEXT NOT NULL CHECK (state IN ('starting', 'running', 'stopped', 'failed')),
    host_port INTEGER NOT NULL CHECK (host_port BETWEEN 1 AND 65535),
    container_port INTEGER NOT NULL CHECK (container_port BETWEEN 1 AND 65535),
    last_observed_state TEXT NOT NULL,
    last_observed_at TEXT NOT NULL,
    exit_code INTEGER,
    observation_reason TEXT,
    removed_at TEXT,
    UNIQUE (id, application_id),
    FOREIGN KEY (deployment_id, application_id)
        REFERENCES deployments(id, application_id),
    -- Retirement is explicit removal evidence and can never contradict a live
    -- lifecycle state.
    CHECK (removed_at IS NULL OR state IN ('starting', 'stopped'))
);

CREATE UNIQUE INDEX one_live_running_runtime_per_application
    ON runtime_instances(application_id)
    WHERE state = 'running' AND removed_at IS NULL;

CREATE UNIQUE INDEX one_live_runtime_endpoint
    ON runtime_instances(host_port)
    WHERE removed_at IS NULL;

CREATE TABLE exposures (
    application_id TEXT PRIMARY KEY REFERENCES applications(id) ON DELETE CASCADE,
    desired_visibility TEXT NOT NULL
        CHECK (desired_visibility IN ('internal', 'public')),
    domain TEXT,
    materialization_state TEXT NOT NULL CHECK (materialization_state IN (
        'not_materialized', 'applying', 'active', 'removing', 'diverged', 'failed'
    )),
    active_runtime_id TEXT,
    configuration_version TEXT,
    last_materialized_at TEXT,
    last_error_code TEXT,
    last_error_message TEXT,
    -- An active route references a runtime owned by the same Application.
    FOREIGN KEY (active_runtime_id, application_id)
        REFERENCES runtime_instances(id, application_id),
    CHECK (desired_visibility = 'internal' OR domain IS NOT NULL),
    -- Confirmed route evidence is all-present or all-absent.
    CHECK (
        (active_runtime_id IS NULL
            AND configuration_version IS NULL
            AND last_materialized_at IS NULL)
        OR (active_runtime_id IS NOT NULL
            AND configuration_version IS NOT NULL
            AND last_materialized_at IS NOT NULL)
    ),
    -- Diagnostics are all-present or all-absent.
    CHECK (
        (last_error_code IS NULL AND last_error_message IS NULL)
        OR (last_error_code IS NOT NULL AND last_error_message IS NOT NULL)
    )
);

CREATE UNIQUE INDEX one_owner_per_public_domain
    ON exposures(lower(domain))
    WHERE desired_visibility = 'public' AND domain IS NOT NULL;

CREATE TABLE runtime_port_reservations (
    port INTEGER PRIMARY KEY CHECK (port BETWEEN 1 AND 65535),
    application_id TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    deployment_id TEXT NOT NULL,
    UNIQUE (deployment_id),
    FOREIGN KEY (deployment_id, application_id)
        REFERENCES deployments(id, application_id)
);
