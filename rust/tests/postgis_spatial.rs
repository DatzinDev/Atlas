use atlas::{
    config::Config,
    domain::Error,
    postgis::{
        connection::Database,
        spatial::{bbox, prepare},
    },
    query::predicate::Spatial,
};
use sqlx::QueryBuilder;

#[tokio::test]
async fn topology_transform_and_indexable_predicates() {
    let config = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").expect("disposable PostGIS required")),
        None,
    )
    .unwrap();
    let db = Database::open(&config).await.unwrap();
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
        assert!(!query.sql().as_str().contains("ST_Transform(geom"));
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
    assert!(matches!(
        prepare(&mut tx, &bbox([0., 91., 1., 92.]).unwrap(), 3857).await,
        Err(Error::InvalidFilter)
    ));
    assert!(matches!(
        prepare(&mut tx, &polygon, 910000).await,
        Err(Error::SchemaChanged)
    ));
    assert!(bbox([2., 0., 1., 1.]).is_err());
    tx.rollback().await.unwrap();
    db.close().await;
}
