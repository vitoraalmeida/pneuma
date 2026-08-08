CREATE TABLE runtime_port_reservations (
    port INTEGER PRIMARY KEY CHECK (port BETWEEN 1 AND 65535),
    application_id TEXT NOT NULL REFERENCES applications(id) ON DELETE CASCADE,
    deployment_id TEXT NOT NULL REFERENCES deployments(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
