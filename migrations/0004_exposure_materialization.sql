ALTER TABLE exposures ADD COLUMN active_runtime_id TEXT
    REFERENCES runtime_instances(id);

ALTER TABLE exposures ADD COLUMN materialization_state TEXT NOT NULL
    DEFAULT 'not_materialized'
    CHECK (materialization_state IN (
        'not_materialized',
        'applying',
        'active',
        'removing',
        'diverged',
        'failed'
    ));

ALTER TABLE exposures ADD COLUMN configuration_version TEXT;
ALTER TABLE exposures ADD COLUMN last_materialized_at TEXT;
ALTER TABLE exposures ADD COLUMN last_error_code TEXT;
ALTER TABLE exposures ADD COLUMN last_error_message TEXT;
