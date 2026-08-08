CREATE TABLE systems (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    description TEXT,
    created_at TEXT NOT NULL
);

ALTER TABLE applications ADD COLUMN system_id TEXT REFERENCES systems(id);
