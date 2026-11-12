CREATE TABLE IF NOT EXISTS atlas_catalog.migrations (
    version integer PRIMARY KEY,
    applied_at timestamptz NOT NULL DEFAULT now()
);
