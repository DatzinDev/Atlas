use atlas::{
    config::Config,
    domain::Error,
    postgis::{
        connection::Database,
        spatial::{Geometry, bbox, prepare},
    },
    query::predicate::Spatial,
};
use sqlx::{Postgres, QueryBuilder, Transaction};

async fn open() -> Database {
    let config = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").expect("disposable PostGIS required")),
        None,
    )
    .unwrap();
    Database::open(&config).await.unwrap()
}

async fn intersecting(
    tx: &mut Transaction<'_, Postgres>,
    table: &str,
    geometry: &Geometry,
) -> Vec<i32> {
    let mut query = QueryBuilder::new(format!("SELECT id FROM {table} WHERE "));
    geometry
        .append(&mut query, "geom", Spatial::Intersects)
        .unwrap();
    assert!(!query.sql().as_str().contains("ST_Transform(geom"));
    query.push(" ORDER BY id");
    query
        .build_query_scalar()
        .fetch_all(&mut **tx)
        .await
        .unwrap()
}

#[tokio::test]
async fn topology_transform_and_indexable_predicates() {
    let db = open().await;
    let mut tx = db.begin_catalog().await.unwrap();
    sqlx::query(
        "CREATE TEMP TABLE spatial_places(id int, geom geometry(Point,3857)) ON COMMIT DROP",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("INSERT INTO spatial_places VALUES (1, ST_Transform(ST_SetSRID(ST_Point(1,1),4326),3857)),(2,ST_Transform(ST_SetSRID(ST_Point(0,0),4326),3857)),(3,ST_Transform(ST_SetSRID(ST_Point(3,3),4326),3857))").execute(&mut *tx).await.unwrap();
    sqlx::query("CREATE INDEX ON spatial_places USING gist(geom)")
        .execute(&mut *tx)
        .await
        .unwrap();
    let polygon = bbox([0., 0., 2., 2.]).unwrap();
    let geometry = prepare(&mut tx, &polygon, 3857).await.unwrap();
    for (op, expected) in [
        (Spatial::Within, vec![1]),
        (Spatial::Intersects, vec![1, 2]),
    ] {
        let mut query = QueryBuilder::new("SELECT id FROM spatial_places WHERE ");
        geometry.append(&mut query, "geom", op).unwrap();
        let sql = query.sql().as_str().to_owned();
        assert!(!sql.contains("ST_Transform(geom"));
        // The EWKB travels once per node; PostGIS >= 3.1 adds the index condition itself.
        assert_eq!(sql.matches("ST_GeomFromEWKB(").count(), 1, "{sql}");
        assert!(!sql.contains("&&"), "{sql}");
        query.push(" ORDER BY id");
        let ids: Vec<i32> = query
            .build_query_scalar()
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        assert_eq!(ids, expected);
    }
    sqlx::query("SET LOCAL enable_seqscan=off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let mut explain = QueryBuilder::new("EXPLAIN SELECT id FROM spatial_places WHERE ");
    geometry
        .append(&mut explain, "geom", Spatial::Intersects)
        .unwrap();
    let plan: Vec<String> = explain
        .build_query_scalar()
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert!(plan.iter().any(|s| s.contains("Index Cond:")), "{plan:?}");
    let mut hole = polygon.clone();
    hole[0].push(vec![
        [0.5, 0.5],
        [1.5, 0.5],
        [1.5, 1.5],
        [0.5, 1.5],
        [0.5, 0.5],
    ]);
    let geometry = prepare(&mut tx, &hole, 3857).await.unwrap();
    let mut query = QueryBuilder::new("SELECT id FROM spatial_places WHERE ");
    geometry
        .append(&mut query, "geom", Spatial::Intersects)
        .unwrap();
    assert_eq!(
        query
            .build_query_scalar::<i32>()
            .fetch_all(&mut *tx)
            .await
            .unwrap(),
        vec![2]
    );
    let bowtie = vec![vec![vec![[0., 0.], [2., 2.], [0., 2.], [2., 0.], [0., 0.]]]];
    assert!(matches!(
        prepare(&mut tx, &bowtie, 4326).await,
        Err(Error::InvalidFilter)
    ));
    assert!(matches!(
        prepare(&mut tx, &[], 4326).await,
        Err(Error::InvalidFilter)
    ));
    assert!(bbox([0., 91., 1., 92.]).is_err());
    let out_of_range = vec![vec![vec![
        [0., 91.],
        [1., 91.],
        [1., 92.],
        [0., 92.],
        [0., 91.],
    ]]];
    assert!(matches!(
        prepare(&mut tx, &out_of_range, 3857).await,
        Err(Error::InvalidFilter)
    ));
    // The SRID check is fused with the transform but still reports a schema change,
    // without aborting the transaction (later prepares below reuse it).
    for srid in [910000, 0, -1] {
        assert!(
            matches!(
                prepare(&mut tx, &polygon, srid).await,
                Err(Error::SchemaChanged)
            ),
            "{srid}"
        );
    }
    assert!(bbox([2., 0., 1., 1.]).is_err());
    let mut exterior_hole = polygon.clone();
    exterior_hole[0].push(bbox([5., 5., 6., 6.]).unwrap().remove(0).remove(0));
    let zero_area = vec![vec![vec![[0., 0.], [0., 1.], [0., 0.5], [0., 0.]]]];
    for invalid in [exterior_hole, zero_area] {
        assert!(matches!(
            prepare(&mut tx, &invalid, 4326).await,
            Err(Error::InvalidFilter)
        ));
    }
    let poles = bbox([0., 85., 1., 90.]).unwrap();
    assert!(prepare(&mut tx, &poles, 4326).await.is_ok());
    // Polygons touching the poles are clipped to the Web Mercator domain, not rejected.
    let geometry = prepare(&mut tx, &poles, 3857).await.unwrap();
    assert!(
        intersecting(&mut tx, "spatial_places", &geometry)
            .await
            .is_empty()
    );
    let world = bbox([-180., -90., 180., 90.]).unwrap();
    let geometry = prepare(&mut tx, &world, 3857).await.unwrap();
    assert_eq!(
        intersecting(&mut tx, "spatial_places", &geometry).await,
        vec![1, 2, 3]
    );
    // Entirely beyond +-85.0511: empty intersection selects nothing and is not an error.
    let polar = bbox([0., 86., 1., 90.]).unwrap();
    let geometry = prepare(&mut tx, &polar, 3857).await.unwrap();
    assert!(
        intersecting(&mut tx, "spatial_places", &geometry)
            .await
            .is_empty()
    );
    let mut multi = polygon.clone();
    multi.extend(bbox([2.5, 2.5, 3.5, 3.5]).unwrap());
    let geometry = prepare(&mut tx, &multi, 3857).await.unwrap();
    let mut query = QueryBuilder::new("SELECT id FROM spatial_places WHERE ");
    geometry
        .append(&mut query, "geom", Spatial::Within)
        .unwrap();
    query.push(" ORDER BY id");
    assert_eq!(
        query
            .build_query_scalar::<i32>()
            .fetch_all(&mut *tx)
            .await
            .unwrap(),
        vec![1, 3]
    );
    tx.rollback().await.unwrap();
    db.close().await;
}

#[tokio::test]
async fn projected_selection_is_densified_and_domain_checked() {
    let db = open().await;
    let mut tx = db.begin_catalog().await.unwrap();
    sqlx::query("CREATE TEMP TABLE utm_places(id int, geom geometry(Point,32614)) ON COMMIT DROP")
        .execute(&mut *tx)
        .await
        .unwrap();
    // Point 1 lies inside the densified bbox but outside the straight-edged transform.
    sqlx::query("INSERT INTO utm_places SELECT 1, ST_PointOnSurface(ST_Difference(ST_Transform(ST_Segmentize(e,1),32614), ST_Transform(e,32614))) FROM ST_MakeEnvelope(-102,18,-96,22,4326) AS e UNION ALL SELECT 2, ST_Transform(ST_SetSRID(ST_Point(-99,20),4326),32614) UNION ALL SELECT 3, ST_Transform(ST_SetSRID(ST_Point(-95,20),4326),32614)")
        .execute(&mut *tx)
        .await
        .unwrap();
    let outside_plain: bool = sqlx::query_scalar("SELECT NOT ST_IsEmpty(geom) AND NOT ST_Intersects(geom, ST_Transform(ST_MakeEnvelope(-102,18,-96,22,4326),32614)) FROM utm_places WHERE id=1")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert!(
        outside_plain,
        "fixture point must fall in the densification gap"
    );
    let geometry = prepare(&mut tx, &bbox([-102., 18., -96., 22.]).unwrap(), 32614)
        .await
        .unwrap();
    assert_eq!(
        intersecting(&mut tx, "utm_places", &geometry).await,
        vec![1, 2]
    );
    // A world polygon collapses outside the UTM zone domain: rejected, never a near-empty match.
    assert!(matches!(
        prepare(&mut tx, &bbox([-180., -90., 180., 90.]).unwrap(), 32614).await,
        Err(Error::InvalidFilter)
    ));
    tx.rollback().await.unwrap();
    db.close().await;
}
