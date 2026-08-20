CREATE TABLE application_operations (
    application_id TEXT PRIMARY KEY REFERENCES applications(id) ON DELETE CASCADE,
    owner_token TEXT NOT NULL,
    generation INTEGER NOT NULL CHECK (generation > 0),
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
