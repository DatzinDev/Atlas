-- Backfill metadata.id_type for publications stored before it existed; idempotent.
-- Rows whose source column no longer resolves are left untouched and read as schema_changed.
UPDATE atlas_catalog.datasets d
SET metadata = d.metadata || jsonb_build_object('id_type', t.typname)
FROM pg_catalog.pg_namespace n
JOIN pg_catalog.pg_class c ON c.relnamespace = n.oid
JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped
JOIN pg_catalog.pg_type t ON t.oid = a.atttypid
WHERE NOT d.metadata ? 'id_type'
  AND n.nspname = d.config #>> '{source,schema}'
  AND c.relname = d.config #>> '{source,table}'
  AND a.attname = d.metadata ->> 'id_column';
