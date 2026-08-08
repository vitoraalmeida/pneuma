CREATE TABLE application_delivery_specs (
    application_id TEXT PRIMARY KEY REFERENCES applications(id) ON DELETE CASCADE,
    delivery_type TEXT NOT NULL CHECK (delivery_type IN ('oci')),
    image_repository TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
