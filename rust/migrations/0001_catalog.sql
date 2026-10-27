CREATE SCHEMA IF NOT EXISTS atlas_catalog;
CREATE TABLE IF NOT EXISTS atlas_catalog.datasets (
    id text PRIMARY KEY,
    config jsonb NOT NULL CHECK (jsonb_typeof(config) = 'object'),
    metadata jsonb NOT NULL CHECK (jsonb_typeof(metadata) = 'object'),
    revision bigint NOT NULL DEFAULT 1 CHECK (revision > 0),
    created_at timestamptz NOT NULL DEFAULT now(),
    updated_at timestamptz NOT NULL DEFAULT now()
);
