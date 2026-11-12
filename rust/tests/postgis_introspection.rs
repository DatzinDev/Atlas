use atlas::{
    config::Config,
    domain::{Dataset, Error, Metadata},
    postgis::{connection::Database, introspection::inspect},
};

// R02
#[tokio::test]
async fn real_relations_geometry_identifiers_and_properties() {
    let config = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required")),
        None,
    )
    .unwrap();
    let db = Database::open(&config).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for statement in [
        "DROP SCHEMA IF EXISTS introspection_test CASCADE",
        "CREATE SCHEMA introspection_test",
        "CREATE TABLE introspection_test.\"quoted\"\"table\"(\"weird\"\"id\" bigint PRIMARY KEY, name text, payload bytea, geom geometry(Point,4326))",
        "CREATE TABLE introspection_test.ambiguous(id integer PRIMARY KEY, geom geometry(Point,4326), other geometry(Point,3857))",
        "CREATE TABLE introspection_test.generic(id integer PRIMARY KEY, geom geometry)",
        "CREATE TABLE introspection_test.composite(id integer, second integer, geom geometry(Point,4326), PRIMARY KEY(id,second))",
        "CREATE TABLE introspection_test.text_id(code text PRIMARY KEY, geom geometry(Point,4326))",
        "CREATE TABLE introspection_test.unknown_crs(id integer PRIMARY KEY, geom geometry(Point,910000))",
        "CREATE VIEW introspection_test.good AS SELECT \"weird\"\"id\" AS id, geom FROM introspection_test.\"quoted\"\"table\"",
        "CREATE VIEW introspection_test.duplicate AS SELECT 1 AS id, ST_SetSRID(ST_Point(0,0),4326)::geometry(Point,4326) AS geom FROM generate_series(1,2)",
        "CREATE VIEW introspection_test.null_id AS SELECT NULL::integer AS id, ST_SetSRID(ST_Point(0,0),4326)::geometry(Point,4326) AS geom",
        "CREATE MATERIALIZED VIEW introspection_test.materialized AS SELECT * FROM introspection_test.good",
    ] {
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let mut config = config;
    config.allowed_schemas = vec!["introspection_test".into()];
    let mut dataset: Dataset = serde_json::from_str(r#"{"id":"test","title":"Test","source":{"schema":"introspection_test","table":"quoted\"table"},"properties":["weird\"id","name"]}"#).unwrap();
    let meta = inspect(&db, &config, &dataset).await.unwrap();
    assert_eq!(meta.srid, 4326);
    assert_eq!(meta.id_column, "weird\"id");
    assert_eq!(meta.attributes.len(), 2);
    assert_eq!(meta.id_type, "int8");
    dataset.properties = vec!["payload".into()];
    assert!(inspect(&db, &config, &dataset).await.is_err());
    dataset.properties = vec!["geom".into()];
    assert!(inspect(&db, &config, &dataset).await.is_err());
    dataset.source.table = "ambiguous".into();
    dataset.properties = vec!["id".into()];
    assert!(inspect(&db, &config, &dataset).await.is_err());
    dataset.source.geometry_column = Some("other".into());
    let meta = inspect(&db, &config, &dataset).await.unwrap();
    assert_eq!((meta.srid, meta.id_type.as_str()), (3857, "int4"));
    dataset.source.geometry_column = None;
    dataset.source.table = "text_id".into();
    dataset.properties = vec!["code".into()];
    assert_eq!(
        inspect(&db, &config, &dataset).await.unwrap().id_type,
        "text"
    );
    dataset.properties = vec!["id".into()];
    for table in [
        "generic",
        "composite",
        "unknown_crs",
        "good",
        "duplicate",
        "null_id",
        "not_exists",
        "x'; DROP SCHEMA public CASCADE--",
    ] {
        dataset.source.table = table.into();
        assert!(inspect(&db, &config, &dataset).await.is_err());
    }
    dataset.source.id_column = Some("id".into());
    for table in ["good", "materialized"] {
        dataset.source.table = table.into();
        assert_eq!(
            inspect(&db, &config, &dataset).await.unwrap().id_column,
            "id"
        );
    }
    for table in ["duplicate", "null_id"] {
        dataset.source.table = table.into();
        assert!(inspect(&db, &config, &dataset).await.is_err());
    }
    dataset.source.schema = "pg_catalog".into();
    assert!(inspect(&db, &config, &dataset).await.is_err());
    dataset.source.schema = "introspection_test".into();
    dataset.source.table = "generic".into();
    dataset.source.id_column = None;
    let mut tx = db.begin_catalog().await.unwrap();
    sqlx::query(
        "INSERT INTO introspection_test.generic VALUES (1, ST_SetSRID(ST_Point(0,0),4326))",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(inspect(&db, &config, &dataset).await.unwrap().srid, 4326);
    let mut tx = db.begin_catalog().await.unwrap();
    sqlx::query("DROP SCHEMA introspection_test CASCADE")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    db.close().await;
}

async fn inspect_relation(db: &Database, config: &Config, table: &str) -> Result<Metadata, Error> {
    let dataset: Dataset = serde_json::from_value(serde_json::json!({
        "id": "test",
        "title": "Test",
        "source": {"schema": "introspection_geometry", "table": table, "id_column": "id"},
        "properties": ["id"],
    }))
    .unwrap();
    inspect(db, config, &dataset).await
}

// R02
#[tokio::test]
async fn geometry_without_typmod_requires_single_type_and_srid() {
    let mut config = Config::parse(
        "[limits]\nquery_seconds=1",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required")),
        None,
    )
    .unwrap();
    config.allowed_schemas = vec!["introspection_geometry".into()];
    let db = Database::open(&config).await.unwrap();
    let point = |srid: i32| format!("ST_SetSRID(ST_Point(0,0),{srid})");
    let wkt = |text: &str| format!("ST_GeomFromText('{text}',4326)");
    let polygon = wkt("POLYGON((0 0,1 0,1 1,0 0))");
    let multipolygon = wkt("MULTIPOLYGON(((0 0,1 0,1 1,0 0)))");
    let line = wkt("LINESTRING(0 0,1 1)");
    let multiline = wkt("MULTILINESTRING((0 0,1 1))");
    let multipoint = wkt("MULTIPOINT((0 0))");
    let collection = wkt("GEOMETRYCOLLECTION(POINT(0 0))");
    let view = |name: &str, geometries: &[&str]| {
        let rows = geometries
            .iter()
            .enumerate()
            .map(|(i, g)| format!("SELECT {} AS id, {g} AS geom", i + 1))
            .collect::<Vec<_>>()
            .join(" UNION ALL ");
        format!("CREATE VIEW introspection_geometry.{name} AS {rows}")
    };
    let p4326 = point(4326);
    let statements = [
        "DROP SCHEMA IF EXISTS introspection_geometry CASCADE".to_string(),
        "CREATE SCHEMA introspection_geometry".into(),
        view("mixed_srid", &[&p4326, &point(3857)]),
        view("srid_and_zero", &[&p4326, &point(0)]),
        view("srid_zero", &[&point(0)]),
        view("unknown_crs", &[&point(910000)]),
        view("polygon_multipolygon", &[&polygon, &multipolygon, &polygon]),
        view("point_multipoint", &[&p4326, &multipoint]),
        view("line_multiline", &[&multiline, &line]),
        view("only_multipolygon", &[&multipolygon]),
        view("only_collection", &[&collection]),
        view("point_line", &[&p4326, &line]),
        view("point_multipolygon", &[&p4326, &multipolygon]),
        view("multipoint_multipolygon", &[&multipoint, &multipolygon]),
        view("point_multipoint_line", &[&p4326, &multipoint, &line]),
        view("point_collection", &[&p4326, &collection]),
        view("point_z", &[&wkt("POINT Z (0 0 0)")]),
        view("point_m", &[&wkt("POINT M (0 0 0)")]),
        view("point_2d_and_z", &[&p4326, &wkt("POINT Z (0 0 0)")]),
        view("with_nulls", &["NULL::geometry", &p4326, "NULL::geometry"]),
        view("only_nulls", &["NULL::geometry"]),
        // Regression: LIMIT 1 sampling saw only the leading 4326 rows.
        "CREATE VIEW introspection_geometry.late_srid AS SELECT g AS id, ST_SetSRID(ST_Point(g,0), CASE WHEN g=5000 THEN 3857 ELSE 4326 END) AS geom FROM generate_series(1,5000) g".into(),
        "CREATE TABLE introspection_geometry.empty_generic(id integer PRIMARY KEY, geom geometry)".into(),
        "CREATE TABLE introspection_geometry.empty_typed(id integer PRIMARY KEY, geom geometry(Point,4326))".into(),
        "CREATE TABLE introspection_geometry.empty_type_only(id integer PRIMARY KEY, geom geometry(Point))".into(),
        "CREATE TABLE introspection_geometry.empty_srid_only(id integer PRIMARY KEY, geom geometry(Geometry,4326))".into(),
        "CREATE TABLE introspection_geometry.typed_z(id integer PRIMARY KEY, geom geometry(PointZ,4326))".into(),
        "CREATE TABLE introspection_geometry.type_only(id integer PRIMARY KEY, geom geometry(Point))".into(),
        "INSERT INTO introspection_geometry.type_only VALUES (1, ST_SetSRID(ST_Point(0,0),4326)), (2, ST_SetSRID(ST_Point(1,1),4326))".into(),
        "CREATE TABLE introspection_geometry.type_only_mixed(id integer PRIMARY KEY, geom geometry(Point))".into(),
        "INSERT INTO introspection_geometry.type_only_mixed VALUES (1, ST_SetSRID(ST_Point(0,0),4326)), (2, ST_SetSRID(ST_Point(0,0),3857))".into(),
        "CREATE TABLE introspection_geometry.srid_only(id integer PRIMARY KEY, geom geometry(Geometry,4326))".into(),
        format!("INSERT INTO introspection_geometry.srid_only VALUES (1, {polygon}), (2, {multipolygon})"),
        "CREATE TABLE introspection_geometry.srid_only_mixed(id integer PRIMARY KEY, geom geometry(Geometry,4326))".into(),
        format!("INSERT INTO introspection_geometry.srid_only_mixed VALUES (1, {p4326}), (2, {line})"),
        "CREATE FUNCTION introspection_geometry.slow_point() RETURNS geometry LANGUAGE sql AS $$ SELECT pg_sleep(5); SELECT ST_SetSRID(ST_Point(0,0),4326) $$".into(),
        "CREATE VIEW introspection_geometry.slow AS SELECT 1 AS id, introspection_geometry.slow_point() AS geom".into(),
    ];
    let mut tx = db.begin_catalog().await.unwrap();
    for statement in &statements {
        sqlx::query(sqlx::AssertSqlSafe(statement.clone()))
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();

    for (table, geometry_type, srid) in [
        ("polygon_multipolygon", "MULTIPOLYGON", 4326),
        ("point_multipoint", "MULTIPOINT", 4326),
        ("line_multiline", "MULTILINESTRING", 4326),
        ("only_multipolygon", "MULTIPOLYGON", 4326),
        ("only_collection", "GEOMETRYCOLLECTION", 4326),
        ("with_nulls", "POINT", 4326),
        ("empty_typed", "POINT", 4326),
        ("type_only", "POINT", 4326),
        ("srid_only", "MULTIPOLYGON", 4326),
    ] {
        let meta = inspect_relation(&db, &config, table)
            .await
            .unwrap_or_else(|e| panic!("{table}: {e:?}"));
        assert_eq!(meta.geometry_type, geometry_type, "{table}");
        assert_eq!(meta.srid, srid, "{table}");
        assert_eq!(meta.geometry_column, "geom", "{table}");
    }
    for table in [
        "mixed_srid",
        "late_srid",
        "srid_and_zero",
        "srid_zero",
        "unknown_crs",
        "point_line",
        "point_multipolygon",
        "multipoint_multipolygon",
        "point_multipoint_line",
        "point_collection",
        "point_z",
        "point_m",
        "point_2d_and_z",
        "only_nulls",
        "empty_generic",
        "empty_type_only",
        "empty_srid_only",
        "typed_z",
        "type_only_mixed",
        "srid_only_mixed",
    ] {
        assert_eq!(
            inspect_relation(&db, &config, table).await.unwrap_err(),
            Error::InvalidRequest,
            "{table}"
        );
    }
    assert_eq!(
        inspect_relation(&db, &config, "slow").await.unwrap_err(),
        Error::QueryTimeout
    );

    let mut tx = db.begin_catalog().await.unwrap();
    sqlx::query("DROP SCHEMA introspection_geometry CASCADE")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    db.close().await;
}

fn dataset_for(schema: &str, table: &str, id: Option<&str>, properties: &[&str]) -> Dataset {
    serde_json::from_value(serde_json::json!({
        "id": "test",
        "title": "Test",
        "source": {"schema": schema, "table": table, "id_column": id},
        "properties": properties,
    }))
    .unwrap()
}

async fn execute_all(db: &Database, statements: &[&str]) {
    let mut tx = db.begin_catalog().await.unwrap();
    for statement in statements {
        sqlx::query(sqlx::AssertSqlSafe(statement.to_string()))
            .execute(&mut *tx)
            .await
            .unwrap_or_else(|e| panic!("{statement}: {e}"));
    }
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn only_builtin_base_types_and_extension_geometry_are_accepted() {
    let mut config = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required")),
        None,
    )
    .unwrap();
    config.allowed_schemas = vec!["introspection_types".into()];
    let db = Database::open(&config).await.unwrap();
    let point = "ST_SetSRID(ST_Point(0,0),4326)::geometry(Point,4326)";
    execute_all(
        &db,
        &[
            "DROP SCHEMA IF EXISTS introspection_types CASCADE",
            "CREATE SCHEMA introspection_types",
            "CREATE TYPE introspection_types.text AS ENUM ('a')",
            "CREATE TYPE introspection_types.int4 AS ENUM ('1')",
            "CREATE DOMAIN introspection_types.d AS pg_catalog.text",
            "CREATE DOMAIN introspection_types.int8 AS pg_catalog.int8",
            "CREATE DOMAIN introspection_types.geometry AS geometry",
            "CREATE TABLE introspection_types.enum_property(id integer PRIMARY KEY, geom geometry(Point,4326), label introspection_types.text)",
            "CREATE TABLE introspection_types.domain_property(id integer PRIMARY KEY, geom geometry(Point,4326), label introspection_types.d)",
            "CREATE TABLE introspection_types.enum_int4_property(id integer PRIMARY KEY, geom geometry(Point,4326), n introspection_types.int4)",
            "CREATE TABLE introspection_types.domain_id(id introspection_types.int8 PRIMARY KEY, geom geometry(Point,4326))",
            "CREATE TABLE introspection_types.text_domain_id(id introspection_types.d PRIMARY KEY, geom geometry(Point,4326))",
            &*format!("CREATE VIEW introspection_types.enum_id AS SELECT 'a'::introspection_types.text AS id, {point} AS geom"),
            &*format!("CREATE VIEW introspection_types.builtin_text_id AS SELECT 'a'::pg_catalog.text AS id, {point} AS geom"),
            "CREATE TABLE introspection_types.fake_geometry(id integer PRIMARY KEY, geom introspection_types.geometry)",
            "INSERT INTO introspection_types.fake_geometry VALUES (1, ST_SetSRID(ST_Point(0,0),4326))",
            "CREATE TABLE introspection_types.builtin(id bigint PRIMARY KEY, geom geometry(Point,4326), a int2, b int4, c float4, d float8, e numeric, f bool, g pg_catalog.text, h varchar(10), i char(2), j date, k timestamp, l timestamptz)",
        ],
    )
    .await;
    let schema = "introspection_types";

    // Regression: types were matched by typname only, so a homonymous enum passed.
    for (table, id, properties) in [
        ("enum_property", None, &["label"][..]),
        ("domain_property", None, &["label"][..]),
        ("enum_int4_property", None, &["n"][..]),
        ("domain_id", None, &[][..]),
        ("text_domain_id", None, &[][..]),
        ("enum_id", Some("id"), &[][..]),
        ("fake_geometry", None, &["id"][..]),
    ] {
        let dataset = dataset_for(schema, table, id, properties);
        assert_eq!(
            inspect(&db, &config, &dataset).await.unwrap_err(),
            Error::InvalidRequest,
            "{table}"
        );
    }

    let dataset = dataset_for(schema, "builtin_text_id", Some("id"), &["id"]);
    let meta = inspect(&db, &config, &dataset).await.unwrap();
    assert_eq!(meta.id_column, "id");
    assert_eq!(meta.attributes[0].data_type, "text");

    let columns = [
        "id", "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l",
    ];
    let dataset = dataset_for(schema, "builtin", None, &columns);
    let meta = inspect(&db, &config, &dataset).await.unwrap();
    assert_eq!(meta.geometry_column, "geom");
    assert_eq!(meta.srid, 4326);
    assert_eq!(
        meta.attributes
            .iter()
            .map(|a| a.data_type.as_str())
            .collect::<Vec<_>>(),
        [
            "int8",
            "int2",
            "int4",
            "float4",
            "float8",
            "numeric",
            "bool",
            "text",
            "varchar",
            "bpchar",
            "date",
            "timestamp",
            "timestamptz",
        ]
    );

    execute_all(&db, &["DROP SCHEMA introspection_types CASCADE"]).await;
    db.close().await;
}

#[tokio::test]
async fn publication_requires_select_on_every_published_column() {
    let dsn = std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required");
    let mut admin_config = Config::parse("", Some(dsn.clone()), None).unwrap();
    admin_config.allowed_schemas = vec!["introspection_privileges".into()];
    let admin = Database::open(&admin_config).await.unwrap();
    let role = "atlas_introspection_reader";
    let table = |name: &str| {
        format!(
            "CREATE TABLE introspection_privileges.{name}(id integer PRIMARY KEY, geom geometry(Point,4326), nombre text, secreto text, payload jsonb)"
        )
    };
    let statements = [
        "DROP SCHEMA IF EXISTS introspection_privileges CASCADE".to_string(),
        format!(
            "DO $$ BEGIN IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname='{role}') THEN CREATE ROLE {role} NOLOGIN; END IF; END $$"
        ),
        format!("GRANT {role} TO CURRENT_USER"),
        "CREATE SCHEMA introspection_privileges".into(),
        format!("GRANT USAGE ON SCHEMA introspection_privileges TO {role}"),
        table("no_select"),
        table("granted"),
        format!("GRANT SELECT (id, geom, nombre) ON introspection_privileges.granted TO {role}"),
        table("no_geometry"),
        format!("GRANT SELECT (id, nombre) ON introspection_privileges.no_geometry TO {role}"),
        table("no_id"),
        format!("GRANT SELECT (geom, nombre) ON introspection_privileges.no_id TO {role}"),
        table("filter_granted"),
        format!(
            "GRANT SELECT (id, geom, nombre, secreto, payload) ON introspection_privileges.filter_granted TO {role}"
        ),
    ];
    execute_all(
        &admin,
        &statements.iter().map(String::as_str).collect::<Vec<_>>(),
    )
    .await;

    let separator = if dsn.contains('?') { '&' } else { '?' };
    let reader_dsn = format!("{dsn}{separator}options=-c%20role%3D{role}");
    let mut config = Config::parse("", Some(reader_dsn), None).unwrap();
    config.allowed_schemas = vec!["introspection_privileges".into()];
    let reader = Database::open(&config).await.unwrap();
    let schema = "introspection_privileges";
    for (table, properties) in [
        ("no_select", &["nombre"][..]),
        ("granted", &["nombre", "secreto"][..]),
        ("granted", &["secreto"][..]),
        ("no_geometry", &["nombre"][..]),
        ("no_id", &["nombre"][..]),
    ] {
        let dataset = dataset_for(schema, table, None, properties);
        assert_eq!(
            inspect(&reader, &config, &dataset).await.unwrap_err(),
            Error::InvalidRequest,
            "{table} {properties:?}"
        );
    }
    for properties in [&["nombre"][..], &["id", "nombre"][..]] {
        let dataset = dataset_for(schema, "granted", None, properties);
        let meta = inspect(&reader, &config, &dataset)
            .await
            .unwrap_or_else(|e| panic!("{properties:?}: {e:?}"));
        assert_eq!(meta.id_column, "id");
        assert_eq!(meta.geometry_column, "geom");
        assert_eq!(meta.attributes.len(), properties.len());
    }
    // The fixed filter may use unpublished columns, but only readable, supported ones.
    let filtered = |table: &str, filter: serde_json::Value| {
        let mut dataset = dataset_for(schema, table, None, &["nombre"]);
        dataset.filter =
            Some(serde_json::value::RawValue::from_string(filter.to_string()).unwrap());
        dataset
    };
    let nested = serde_json::json!({"op": "and", "args": [
        {"op": "eq", "field": "secreto", "value": "x"},
        {"op": "or", "args": [{"op": "is_null", "field": "secreto"},
            {"op": "eq", "field": "nombre", "value": "x"}]}]});
    let meta = inspect(&reader, &config, &filtered("filter_granted", nested))
        .await
        .unwrap();
    assert_eq!(meta.attributes.len(), 1);
    assert_eq!(meta.filter_attributes.len(), 1);
    assert_eq!(meta.filter_attributes[0].name, "secreto");
    for (table, field) in [
        ("granted", "secreto"),
        ("filter_granted", "missing"),
        ("filter_granted", "payload"),
        ("filter_granted", "geom"),
    ] {
        let dataset = filtered(table, serde_json::json!({"op": "is_null", "field": field}));
        assert_eq!(
            inspect(&reader, &config, &dataset).await.unwrap_err(),
            Error::InvalidRequest,
            "{table} {field}"
        );
    }
    reader.close().await;

    execute_all(
        &admin,
        &[
            "DROP SCHEMA introspection_privileges CASCADE",
            &*format!("DROP ROLE IF EXISTS {role}"),
        ],
    )
    .await;
    admin.close().await;
}

/// Asserts a 4326 extent inside the world range that covers `inner` with at most `slack` degrees.
fn assert_covers(extent: Option<[f64; 4]>, inner: [f64; 4], slack: f64, label: &str) {
    let [minx, miny, maxx, maxy] = extent.unwrap_or_else(|| panic!("{label}: no extent"));
    assert!(
        (-180.0..=180.0).contains(&minx)
            && (-180.0..=180.0).contains(&maxx)
            && (-90.0..=90.0).contains(&miny)
            && (-90.0..=90.0).contains(&maxy)
            && minx <= maxx
            && miny <= maxy,
        "{label}: {extent:?}"
    );
    assert!(
        minx <= inner[0] && miny <= inner[1] && maxx >= inner[2] && maxy >= inner[3],
        "{label}: {extent:?} does not cover {inner:?}"
    );
    assert!(
        minx >= inner[0] - slack
            && miny >= inner[1] - slack
            && maxx <= inner[2] + slack
            && maxy <= inner[3] + slack,
        "{label}: {extent:?} is too loose around {inner:?}"
    );
}

#[tokio::test]
async fn extent_is_a_clamped_epsg4326_snapshot_of_the_published_geometry() {
    let mut config = Config::parse(
        "[limits]\nquery_seconds=1",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required")),
        None,
    )
    .unwrap();
    let schema = "introspection_extent";
    config.allowed_schemas = vec![schema.into()];
    let db = Database::open(&config).await.unwrap();
    // Autovacuum is disabled so "no statistics" stays true until an explicit ANALYZE.
    let off = "WITH (autovacuum_enabled=false)";
    let points = "(1, ST_SetSRID(ST_Point(-100,19),4326)), (2, ST_SetSRID(ST_Point(-98,21),4326))";
    let utm = |x: i32, y: i32| format!("ST_Transform(ST_SetSRID(ST_Point({x},{y}),4326),32614)");
    let statements = [
        "DROP SCHEMA IF EXISTS introspection_extent CASCADE".to_string(),
        "CREATE SCHEMA introspection_extent".into(),
        format!("CREATE TABLE introspection_extent.analyzed(id integer PRIMARY KEY, geom geometry(Point,4326)) {off}"),
        format!("INSERT INTO introspection_extent.analyzed VALUES {points}"),
        "ANALYZE introspection_extent.analyzed".into(),
        "CREATE VIEW introspection_extent.over_analyzed AS SELECT id, geom FROM introspection_extent.analyzed".into(),
        format!("CREATE TABLE introspection_extent.no_stats(id integer PRIMARY KEY, geom geometry(Point,4326)) {off}"),
        format!("INSERT INTO introspection_extent.no_stats VALUES {points}"),
        format!("CREATE MATERIALIZED VIEW introspection_extent.materialized {off} AS SELECT id, geom FROM introspection_extent.no_stats"),
        format!("CREATE TABLE introspection_extent.utm(id integer PRIMARY KEY, geom geometry(Point,32614)) {off}"),
        format!("INSERT INTO introspection_extent.utm VALUES (1, {}), (2, {})", utm(-100, 19), utm(-98, 21)),
        format!("CREATE TABLE introspection_extent.out_of_range(id integer PRIMARY KEY, geom geometry(Point,4326)) {off}"),
        "INSERT INTO introspection_extent.out_of_range VALUES (1, ST_SetSRID(ST_Point(-190,-95),4326)), (2, ST_SetSRID(ST_Point(190,95),4326))".into(),
        // ST_EstimatedExtent takes names as text: quoting them as identifiers would break here.
        format!("CREATE TABLE introspection_extent.\"odd\"\"name\"(id integer PRIMARY KEY, \"ge\"\"om\" geometry(Point,4326)) {off}"),
        format!("INSERT INTO introspection_extent.\"odd\"\"name\" VALUES {points}"),
        "ANALYZE introspection_extent.\"odd\"\"name\"".into(),
        format!("CREATE TABLE introspection_extent.empty(id integer PRIMARY KEY, geom geometry(Point,4326)) {off}"),
        "CREATE VIEW introspection_extent.empty_view AS SELECT id, geom FROM introspection_extent.empty".into(),
        // STABLE lets the planner skip the unused geometry, so only the extent waits on it.
        "CREATE FUNCTION introspection_extent.slow_point() RETURNS geometry LANGUAGE sql STABLE AS $$ SELECT pg_sleep(5); SELECT ST_SetSRID(ST_Point(0,0),4326) $$".into(),
        "CREATE VIEW introspection_extent.slow AS SELECT 1 AS id, introspection_extent.slow_point()::geometry(Point,4326) AS geom".into(),
    ];
    execute_all(
        &db,
        &statements.iter().map(String::as_str).collect::<Vec<_>>(),
    )
    .await;
    let (shared_db, shared_config) = (&db, &config);
    let extent = move |table: &'static str, id: Option<&'static str>| async move {
        inspect(
            shared_db,
            shared_config,
            &dataset_for(schema, table, id, &["id"]),
        )
        .await
        .unwrap_or_else(|e| panic!("{table}: {e:?}"))
        .extent
    };
    let expected = [-100.0, 19.0, -98.0, 21.0];

    // Estimated extent after ANALYZE: may be looser, but must cover the data.
    assert_covers(extent("analyzed", None).await, expected, 1.0, "analyzed");
    assert_covers(
        inspect(
            &db,
            &config,
            &dataset_for(schema, "odd\"name", None, &["id"]),
        )
        .await
        .unwrap()
        .extent,
        expected,
        1.0,
        "odd\"name",
    );
    // Views use ST_Extent; tables without statistics fall back to it: both exact.
    assert_eq!(extent("over_analyzed", Some("id")).await, Some(expected));
    assert_eq!(extent("no_stats", None).await, Some(expected));
    assert_eq!(extent("materialized", Some("id")).await, Some(expected));
    // Projected data is transformed to 4326 by PostGIS.
    let eps = 1e-6;
    assert_covers(
        extent("utm", None).await,
        [-100.0 + eps, 19.0 + eps, -98.0 - eps, 21.0 - eps],
        0.5,
        "utm",
    );
    assert_eq!(
        extent("out_of_range", None).await,
        Some([-180.0, -90.0, 180.0, 90.0])
    );
    // No rows means no extent, not an error.
    assert_eq!(extent("empty", None).await, None);
    assert_eq!(extent("empty_view", Some("id")).await, None);
    // The extent runs under statement_timeout.
    assert_eq!(
        inspect(
            &db,
            &config,
            &dataset_for(schema, "slow", Some("id"), &["id"])
        )
        .await
        .unwrap_err(),
        Error::QueryTimeout
    );

    execute_all(&db, &["DROP SCHEMA introspection_extent CASCADE"]).await;
    db.close().await;
}

/// Full sequential scans and tuples read (heap and index) of one relation, flushed first.
async fn reads(db: &Database, schema: &str, relation: &str) -> (i64, i64) {
    // The pool holds one connection, so the flush covers every earlier statement.
    let mut tx = db.begin_read().await.unwrap();
    sqlx::query("SELECT pg_stat_force_next_flush()")
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = db.begin_read().await.unwrap();
    let row: (i64, i64) = sqlx::query_as("SELECT t.seq_scan, t.seq_tup_read + coalesce((SELECT sum(i.idx_tup_read) FROM pg_stat_user_indexes i WHERE i.relid = t.relid), 0)::int8 FROM pg_stat_user_tables t WHERE t.schemaname = $1 AND t.relname = $2")
        .bind(schema)
        .bind(relation)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    row
}

#[tokio::test]
async fn unique_index_on_materialized_view_avoids_the_full_uniqueness_scan() {
    let mut config = Config::parse(
        // One reserved connection: pool_max - data_operations = 1.
        "[limits]\npool_min=1\npool_max=2\ndata_operations=1\nexport_operations=1",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required")),
        None,
    )
    .unwrap();
    let schema = "introspection_unique";
    config.allowed_schemas = vec![schema.into()];
    let db = Database::open(&config).await.unwrap();
    let point = "ST_SetSRID(ST_Point(0,0),4326)::geometry(Point,4326) AS geom";
    let big = "SELECT g AS id, g % 2 AS k, ST_SetSRID(ST_Point(g % 360 - 180, g % 180 - 90),4326)::geometry(Point,4326) AS geom FROM generate_series(1,20000) g";
    let pairs = |values: &str| format!("SELECT v.id, v.k, {point} FROM (VALUES {values}) v(id, k)");
    let mut statements = vec![
        "DROP SCHEMA IF EXISTS introspection_unique CASCADE".to_string(),
        "CREATE SCHEMA introspection_unique".into(),
        format!("CREATE MATERIALIZED VIEW introspection_unique.indexed AS {big}"),
        "CREATE UNIQUE INDEX ON introspection_unique.indexed(id)".into(),
        format!("CREATE MATERIALIZED VIEW introspection_unique.unindexed AS {big}"),
        format!(
            "CREATE MATERIALIZED VIEW introspection_unique.null_indexed AS {big} UNION ALL SELECT NULL, 0, {point}"
        ),
        "CREATE UNIQUE INDEX ON introspection_unique.null_indexed(id)".into(),
        // Statistics make the extent an estimate, so only the uniqueness check can read rows.
        "ANALYZE introspection_unique.indexed".into(),
        "ANALYZE introspection_unique.unindexed".into(),
    ];
    // Each fixture holds a duplicate `id`; the index, if any, never proves `id` unique.
    for (name, values, index) in [
        ("duplicate", "(1,0),(1,1)", ""),
        ("duplicate_plain_index", "(1,0),(1,1)", "INDEX ON {} (id)"),
        (
            "duplicate_partial",
            "(1,0),(1,1),(2,0)",
            "UNIQUE INDEX ON {} (id) WHERE id > 1",
        ),
        (
            "duplicate_composite",
            "(1,0),(1,1)",
            "UNIQUE INDEX ON {} (id, k)",
        ),
        (
            "duplicate_expression",
            "(1,0),(1,1)",
            "UNIQUE INDEX ON {} ((id + k))",
        ),
        ("unique_other", "(1,0),(1,1)", "UNIQUE INDEX ON {} (k)"),
        ("plain_view", "(1,0),(1,1)", ""),
    ] {
        let table = format!("introspection_unique.{name}");
        let kind = if name == "plain_view" {
            "VIEW"
        } else {
            "MATERIALIZED VIEW"
        };
        statements.push(format!("CREATE {kind} {table} AS {}", pairs(values)));
        if !index.is_empty() {
            statements.push(format!("CREATE {}", index.replace("{}", &table)));
        }
    }
    execute_all(
        &db,
        &statements.iter().map(String::as_str).collect::<Vec<_>>(),
    )
    .await;
    let (shared_db, shared_config) = (&db, &config);
    let publish = move |table: &'static str| async move {
        inspect(
            shared_db,
            shared_config,
            &dataset_for(schema, table, Some("id"), &["k"]),
        )
        .await
    };

    // A unique, valid, total index on `id`: only the NULL probe runs, no full walk.
    let before = reads(&db, schema, "indexed").await;
    assert_eq!(publish("indexed").await.unwrap().id_column, "id");
    let after = reads(&db, schema, "indexed").await;
    assert_eq!(after.0, before.0, "sequential scans on an indexed view");
    assert!(
        after.1 - before.1 < 2000,
        "tuples read: {before:?} -> {after:?}"
    );

    // Control: without the index the full scan still runs, so the counters do move.
    let before = reads(&db, schema, "unindexed").await;
    assert_eq!(publish("unindexed").await.unwrap().id_column, "id");
    let after = reads(&db, schema, "unindexed").await;
    assert!(after.0 > before.0, "{before:?} -> {after:?}");

    // A unique index allows several NULLs: the NULL id is still rejected.
    assert_eq!(
        publish("null_indexed").await.unwrap_err(),
        Error::InvalidRequest
    );
    // Duplicates are caught by the full scan whenever the index does not prove uniqueness of `id`.
    for table in [
        "duplicate",
        "duplicate_plain_index",
        "duplicate_partial",
        "duplicate_composite",
        "duplicate_expression",
        "unique_other",
        "plain_view",
    ] {
        assert_eq!(
            publish(table).await.unwrap_err(),
            Error::InvalidRequest,
            "{table}"
        );
    }

    execute_all(&db, &["DROP SCHEMA introspection_unique CASCADE"]).await;
    db.close().await;
}
