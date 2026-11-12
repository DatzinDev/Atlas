use atlas::{
    config::{Config, Limits},
    domain::{Dataset, Error, Metadata},
    postgis::{connection::Database, introspection::inspect, selection::prepare},
    query::predicate::{Predicate, parse},
};
use sqlx::QueryBuilder;

#[tokio::test]
async fn fixed_user_and_bbox_cannot_widen_each_other() {
    // The API parses each filter once; selection only receives the parsed predicates.
    let p = |text: &str| -> Predicate { parse(text, &Limits::default()).unwrap() };
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
        "CREATE TABLE selection_test.places(id int PRIMARY KEY, category text, geom geometry(Point,3857), tenant_id int4)",
        "INSERT INTO selection_test.places SELECT id,category,ST_Transform(ST_SetSRID(ST_Point(x,y),4326),3857),id % 2 FROM (VALUES (1,'a',1,1),(2,'a',3,3),(3,'b',1,1),(4,'a',0,0)) AS v(id,category,x,y)",
    ] {
        sqlx::query(sql).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let dataset: Dataset = serde_json::from_str(r#"{"id":"places","title":"Places","source":{"schema":"selection_test","table":"places"},"properties":["category"]}"#).unwrap();
    let meta = inspect(&db, &config, &dataset).await.unwrap();
    let fixed = p(r#"{"op":"eq","field":"category","value":"a"}"#);
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
        let user = p(user);
        let selection = prepare(
            &mut tx,
            Some(fixed.clone()),
            Some(user),
            Some([0., 0., 2., 2.]),
            &meta,
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
        let bad = p(bad);
        assert_eq!(
            prepare(&mut tx, Some(fixed.clone()), Some(bad), None, &meta)
                .await
                .err(),
            Some(Error::InvalidFilter)
        );
    }
    // A geometry rejected by PostGIS aborts the transaction; check the clean case in a new one.
    tx.rollback().await.unwrap();
    let mut tx = db.begin_read().await.unwrap();
    // An invalid bbox stays a request error and is rejected before touching PostGIS.
    assert_eq!(
        prepare(
            &mut tx,
            Some(fixed.clone()),
            None,
            Some([2., 0., 1., 1.]),
            &meta
        )
        .await
        .err(),
        Some(Error::InvalidRequest)
    );
    let selection = prepare(&mut tx, None, None, None, &meta).await.unwrap();
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

    // The fixed filter may use the unpublished tenant_id; user filters may not.
    let tenant_text = r#"{"op":"eq","field":"tenant_id","value":1}"#;
    let tenant = p(tenant_text);
    let dataset: Dataset = serde_json::from_str(&format!(r#"{{"id":"places","title":"Places","source":{{"schema":"selection_test","table":"places"}},"properties":["category"],"filter":{tenant_text}}}"#)).unwrap();
    let meta = inspect(&db, &config, &dataset).await.unwrap();
    let a = p(r#"{"op":"eq","field":"category","value":"a"}"#);
    // One parsed predicate is reused across several selections and is never re-parsed from text.
    for (user, expected) in [
        (None, vec![1, 3]),
        (Some(a.clone()), vec![1]),
        (Some(a.clone()), vec![1]),
    ] {
        let mut tx = db.begin_read().await.unwrap();
        let selection = prepare(&mut tx, Some(tenant.clone()), user.clone(), None, &meta)
            .await
            .unwrap();
        let mut builder = QueryBuilder::new("SELECT id FROM selection_test.places WHERE ");
        selection.append(&mut builder).unwrap();
        builder.push(" ORDER BY id");
        let found = builder
            .build_query_scalar::<i32>()
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        assert_eq!(found, expected, "{user:?}");
        tx.rollback().await.unwrap();
    }
    let legacy = Metadata {
        filter_attributes: Vec::new(),
        ..meta.clone()
    };
    let mut tx = db.begin_read().await.unwrap();
    for (fixed, user, meta) in [
        (None, Some(tenant.clone()), &meta),
        (Some(tenant.clone()), None, &legacy),
    ] {
        let result = prepare(&mut tx, fixed, user.clone(), None, meta).await;
        assert_eq!(result.err(), Some(Error::InvalidFilter), "{user:?}");
    }
    tx.rollback().await.unwrap();
    db.close().await;
}

#[tokio::test]
async fn each_geometry_stays_bound_to_its_own_node() {
    let p = |text: &str| -> Predicate { parse(text, &Limits::default()).unwrap() };
    let mut config = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").expect("disposable PostGIS required")),
        None,
    )
    .unwrap();
    config.allowed_schemas = vec!["selection_binding_test".into()];
    let db = Database::open(&config).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    // 1 in A, 2 in B, 3 outside both, 4 outside the bbox, 5 in A but outside the fixed area D.
    for sql in [
        "DROP SCHEMA IF EXISTS selection_binding_test CASCADE",
        "CREATE SCHEMA selection_binding_test",
        "CREATE TABLE selection_binding_test.places(id int PRIMARY KEY, label text, geom geometry(Point,4326))",
        "INSERT INTO selection_binding_test.places SELECT id, 'p' || id, ST_SetSRID(ST_Point(x,y),4326) FROM (VALUES (1,1,1),(2,5,1),(3,8,1),(4,1,5),(5,1.8,0.2)) AS v(id,x,y)",
        "CREATE INDEX binding_geom_gist ON selection_binding_test.places USING gist(geom)",
        "ANALYZE selection_binding_test.places",
    ] {
        sqlx::query(sql).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let dataset: Dataset = serde_json::from_str(r#"{"id":"places","title":"Places","source":{"schema":"selection_binding_test","table":"places"},"properties":["label"]}"#).unwrap();
    let meta = inspect(&db, &config, &dataset).await.unwrap();
    let polygon = |[x0, y0, x1, y1]: [f64; 4]| {
        format!(
            r#"{{"type":"Polygon","coordinates":[[[{x0},{y0}],[{x1},{y0}],[{x1},{y1}],[{x0},{y1}],[{x0},{y0}]]]}}"#
        )
    };
    let (a, b, d) = ([0., 0., 2., 2.], [4., 0., 6., 2.], [-1., 0.5, 7., 3.]);
    let bbox = [-1., -1., 10., 3.];
    let envelope =
        |[x0, y0, x1, y1]: [f64; 4]| format!("ST_MakeEnvelope({x0},{y0},{x1},{y1},4326)");
    let user = |a: [f64; 4], b: [f64; 4]| {
        p(&format!(
            r#"{{"op":"or","args":[{{"op":"intersects","geometry":{}}},{{"op":"not","arg":{{"op":"within","geometry":{}}}}}]}}"#,
            polygon(a),
            polygon(b)
        ))
    };
    let fixed = p(&format!(r#"{{"op":"within","geometry":{}}}"#, polygon(d)));
    // Swapping A and B, or dropping the fixed area, changes the expected ids.
    for (fixed, a, b, expected) in [
        (None, a, b, vec![1, 3, 5]),
        (None, b, a, vec![2, 3]),
        (Some(fixed.clone()), a, b, vec![1]),
        (Some(fixed.clone()), b, a, vec![2]),
    ] {
        let mut tx = db.begin_read().await.unwrap();
        let oracle = format!(
            "SELECT id FROM selection_binding_test.places WHERE (ST_Intersects(geom,{}) OR NOT ST_Within(geom,{})) AND ST_Intersects(geom,{}) {} ORDER BY id",
            envelope(a),
            envelope(b),
            envelope(bbox),
            if fixed.is_some() {
                format!("AND ST_Within(geom,{})", envelope(d))
            } else {
                String::new()
            }
        );
        let oracle: Vec<i32> = sqlx::query_scalar(sqlx::AssertSqlSafe(oracle.as_str()))
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        assert_eq!(oracle, expected, "oracle must discriminate the binding");
        let spatial_nodes = 2 + usize::from(fixed.is_some());
        let selection = prepare(&mut tx, fixed, Some(user(a, b)), Some(bbox), &meta)
            .await
            .unwrap();
        let mut builder = QueryBuilder::new("SELECT id FROM selection_binding_test.places WHERE ");
        selection.append(&mut builder).unwrap();
        builder.push(" ORDER BY id");
        let sql = builder.sql().as_str().to_owned();
        // One EWKB per spatial node plus one for the bbox, and no explicit bbox operator.
        assert_eq!(
            sql.matches("ST_GeomFromEWKB(").count(),
            spatial_nodes + 1,
            "{sql}"
        );
        assert!(!sql.contains("&&"), "{sql}");
        let found: Vec<i32> = builder
            .build_query_scalar()
            .fetch_all(&mut *tx)
            .await
            .unwrap();
        assert_eq!(found, oracle);
        tx.rollback().await.unwrap();
    }
    // The selection's intersects node alone is enough for the planner to use the GiST index.
    let mut tx = db.begin_read().await.unwrap();
    sqlx::query("SET LOCAL enable_seqscan = off")
        .execute(&mut *tx)
        .await
        .unwrap();
    let intersects = p(&format!(
        r#"{{"op":"intersects","geometry":{}}}"#,
        polygon(a)
    ));
    let selection = prepare(&mut tx, None, Some(intersects), None, &meta)
        .await
        .unwrap();
    let mut explain =
        QueryBuilder::new("EXPLAIN SELECT id FROM selection_binding_test.places WHERE ");
    selection.append(&mut explain).unwrap();
    let plan: Vec<String> = explain
        .build_query_scalar()
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert!(
        plan.iter().any(|line| line.contains("binding_geom_gist"))
            && plan.iter().any(|line| line.contains("Index Cond:")),
        "{plan:?}"
    );
    tx.rollback().await.unwrap();
    db.close().await;
}
