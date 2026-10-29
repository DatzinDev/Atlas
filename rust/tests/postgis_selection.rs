use atlas::{
    config::{Config, Limits},
    domain::Dataset,
    postgis::{connection::Database, introspection::inspect, selection::prepare},
};
use sqlx::QueryBuilder;

#[tokio::test]
async fn fixed_user_and_bbox_cannot_widen_each_other() {
    let mut config = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").expect("disposable PostGIS required")),
        None,
    )
    .unwrap();
    config.allowed_schemas = vec!["selection_test".into()];
    let db = Database::open(&config).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for sql in [
        "DROP SCHEMA IF EXISTS selection_test CASCADE",
        "CREATE SCHEMA selection_test",
        "CREATE TABLE selection_test.places(id int PRIMARY KEY, category text, geom geometry(Point,3857))",
        "INSERT INTO selection_test.places SELECT id,category,ST_Transform(ST_SetSRID(ST_Point(x,y),4326),3857) FROM (VALUES (1,'a',1,1),(2,'a',3,3),(3,'b',1,1),(4,'a',0,0)) AS v(id,category,x,y)",
    ] {
        sqlx::query(sql).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let dataset: Dataset = serde_json::from_str(r#"{"id":"places","title":"Places","source":{"schema":"selection_test","table":"places"},"properties":["category"]}"#).unwrap();
    let meta = inspect(&db, &config, &dataset).await.unwrap();
    let fixed = r#"{"op":"eq","field":"category","value":"a"}"#;
    let spatial = r#"{"op":"within","geometry":{"type":"Polygon","coordinates":[[[0,0],[2,0],[2,2],[0,2],[0,0]]]}}"#;
    let mixed =
        format!(r#"{{"op":"or","args":[{{"op":"eq","field":"category","value":"b"}},{spatial}]}}"#);
    let not = format!(r#"{{"op":"not","arg":{spatial}}}"#);
    for (user, expected) in [
        (spatial, vec![1]),
        (mixed.as_str(), vec![1]),
        (not.as_str(), vec![4]),
    ] {
        let mut tx = db.begin_read().await.unwrap();
        let selection = prepare(
            &mut tx,
            Some(fixed),
            Some(user),
            Some([0., 0., 2., 2.]),
            &meta,
            &Limits::default(),
        )
        .await
        .unwrap();
        let mut builder = QueryBuilder::new("SELECT id FROM selection_test.places WHERE ");
        selection.append(&mut builder).unwrap();
        builder.push(" ORDER BY id");
        assert_eq!(
            builder
                .build_query_scalar::<i32>()
                .fetch_all(&mut *tx)
                .await
                .unwrap(),
            expected
        );
        tx.rollback().await.unwrap();
    }
    let mut tx = db.begin_read().await.unwrap();
    for bad in [
        r#"{"op":"is_null","field":"not_published"}"#,
        r#"{"op":"intersects","geometry":{"type":"Polygon","coordinates":[[[0,0],[2,2],[0,2],[2,0],[0,0]]]}}"#,
    ] {
        assert!(
            prepare(
                &mut tx,
                Some(fixed),
                Some(bad),
                None,
                &meta,
                &Limits::default()
            )
            .await
            .is_err()
        );
    }
    let selection = prepare(&mut tx, None, None, None, &meta, &Limits::default())
        .await
        .unwrap();
    let mut builder = QueryBuilder::new("SELECT count(*) FROM selection_test.places WHERE ");
    selection.append(&mut builder).unwrap();
    assert_eq!(
        builder
            .build_query_scalar::<i64>()
            .fetch_one(&mut *tx)
            .await
            .unwrap(),
        4
    );
    tx.rollback().await.unwrap();
    db.close().await;
}
