-- Development dataset for Atlas.
-- Runs automatically on the first boot of the postgis container.
-- Geometries are synthetic but valid, roughly placed over central-northern Mexico so that
-- EPSG:4326 and EPSG:32613 (UTM zone 13N) cover the same area.

CREATE EXTENSION IF NOT EXISTS postgis;

CREATE SCHEMA IF NOT EXISTS geo;

-- Polygons in EPSG:4326.
CREATE TABLE geo.municipios (
    id        serial PRIMARY KEY,
    clave     text NOT NULL UNIQUE,
    nombre    text NOT NULL,
    poblacion integer NOT NULL,
    geom      geometry(Polygon, 4326) NOT NULL
);

INSERT INTO geo.municipios (clave, nombre, poblacion, geom) VALUES
    ('001', 'San Bernardo',   250000, ST_GeomFromText('POLYGON((-103.0 21.0, -102.5 21.0, -102.5 21.5, -103.0 21.5, -103.0 21.0))', 4326)),
    ('002', 'Villa Alta',     180000, ST_GeomFromText('POLYGON((-102.5 21.0, -102.0 21.0, -102.0 21.5, -102.5 21.5, -102.5 21.0))', 4326)),
    ('003', 'Los Encinos',     95000, ST_GeomFromText('POLYGON((-103.0 20.5, -102.5 20.5, -102.5 21.0, -103.0 21.0, -103.0 20.5))', 4326)),
    ('004', 'Santa Rita',      42000, ST_GeomFromText('POLYGON((-102.5 20.5, -102.0 20.5, -102.0 21.0, -102.5 21.0, -102.5 20.5))', 4326)),
    ('005', 'Puerto Viejo',   310000, ST_GeomFromText('POLYGON((-102.0 20.5, -101.5 20.5, -101.5 21.5, -102.0 21.5, -102.0 20.5))', 4326));

CREATE INDEX municipios_geom_idx ON geo.municipios USING GIST (geom);

-- Lines in EPSG:4326.
CREATE TABLE geo.carreteras (
    id     serial PRIMARY KEY,
    nombre text NOT NULL,
    tipo   text NOT NULL,
    geom   geometry(LineString, 4326) NOT NULL
);

-- A small but plausible road network: an east-west trunk, a north-south trunk crossing it once,
-- a bypass arc around that junction, and a rural spur feeding into the trunk.
INSERT INTO geo.carreteras (nombre, tipo, geom) VALUES
    ('Federal 45',        'federal',    ST_GeomFromText('LINESTRING(-103.0 21.02, -102.6 21.0, -102.25 20.98, -101.8 20.96, -101.5 20.95)', 4326)),
    ('Estatal 12',        'estatal',    ST_GeomFromText('LINESTRING(-102.25 21.5, -102.24 21.25, -102.25 20.98, -102.28 20.7, -102.3 20.5)', 4326)),
    ('Libramiento Norte', 'estatal',    ST_GeomFromText('LINESTRING(-102.6 21.0, -102.55 21.2, -102.4 21.32, -102.15 21.33, -102.0 21.2, -101.98 21.02)', 4326)),
    ('Camino Viejo',      'terraceria', ST_GeomFromText('LINESTRING(-102.85 20.6, -102.7 20.72, -102.55 20.8, -102.4 20.9, -102.28 20.97)', 4326));

CREATE INDEX carreteras_geom_idx ON geo.carreteras USING GIST (geom);

-- Points in EPSG:32613 on purpose: exercises reprojection against the 4326 layers above.
-- The coordinates are the UTM 13N projection of points that fall INSIDE the municipios extent,
-- so any bbox filter or spatial join across layers has real overlap to work with.
CREATE TABLE geo.escuelas (
    id     serial PRIMARY KEY,
    nombre text NOT NULL,
    nivel  text NOT NULL,
    geom   geometry(Point, 32613) NOT NULL
);

INSERT INTO geo.escuelas (nombre, nivel, geom) VALUES
    ('Primaria Benito Juarez',   'primaria',    ST_SetSRID(ST_MakePoint(728225, 2356942), 32613)),
    ('Secundaria Tecnica 4',     'secundaria',  ST_SetSRID(ST_MakePoint(780309, 2346672), 32613)),
    ('Preparatoria del Valle',   'media',       ST_SetSRID(ST_MakePoint(749815, 2301872), 32613)),
    ('Primaria Ignacio Zaragoza','primaria',    ST_SetSRID(ST_MakePoint(802089, 2291650), 32613)),
    ('Secundaria Sor Juana',     'secundaria',  ST_SetSRID(ST_MakePoint(837678, 2336665), 32613));

CREATE INDEX escuelas_geom_idx ON geo.escuelas USING GIST (geom);

-- View, to exercise introspection of relations without a primary key.
CREATE VIEW geo.v_municipios_grandes AS
SELECT id, clave, nombre, poblacion, geom
FROM geo.municipios
WHERE poblacion > 100000;
