use atlas::{
    config::{Config, Limits},
    domain::{Dataset, Error, Metadata, Query},
    postgis::{
        connection::Database,
        features::{Page, page},
        introspection::inspect,
        selection::prepare,
    },
};
use serde_json::Value;

async fn run(db: &Database, source: &(Dataset, Metadata), query: &str) -> Result<Page, Error> {
    let (dataset, meta) = source;
    let query: Query = serde_json::from_str(query).unwrap();
    let limits = Limits::default();
    let mut tx = db.begin_read().await?;
    let fixed = dataset.filter.as_ref().map(|f| f.get());
    let user = query.filter.as_ref().map(|f| f.get());
    let selection = prepare(&mut tx, fixed, user, query.bbox, meta, &limits).await?;
    page(&mut tx, dataset, meta, &selection, &query, &limits).await
}

fn features(page: &Page) -> Vec<Value> {
    page.features
        .iter()
        .map(|f| serde_json::from_str(f.get()).unwrap())
        .collect()
}

fn ids(page: &Page) -> Vec<i64> {
    features(page)
        .iter()
        .map(|f| f["id"].as_i64().unwrap())
        .collect()
}

async fn setup() -> (Database, (Dataset, Metadata), (Dataset, Metadata)) {
    let mut config = Config::parse(
        "",
        Some(std::env::var("ATLAS_TEST_DATABASE_URL").expect("real disposable PostGIS required")),
        None,
    )
    .unwrap();
    config.allowed_schemas = vec!["features_test".into()];
    let db = Database::open(&config).await.unwrap();
    let mut tx = db.begin_catalog().await.unwrap();
    for statement in [
        "DROP SCHEMA IF EXISTS features_test CASCADE",
        "CREATE SCHEMA features_test",
        "CREATE TABLE features_test.places(id integer PRIMARY KEY, \"we\"\"ird\" text, big int8, amount numeric, rank int4, secret text, geom geometry(Point,3857))",
        "INSERT INTO features_test.places SELECT n, 'p' || n, 9223372036854775807, 12345678901234567890.123, (ARRAY[10,20,20,NULL,10,NULL,30])[n], 'x', ST_Transform(ST_SetSRID(ST_Point(-103.35 + n, 20.67), 4326), 3857) FROM generate_series(1, 7) AS n",
        "CREATE VIEW features_test.places_view AS SELECT id * 10 AS vid, rank, geom FROM features_test.places",
    ] {
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    let mut sources = Vec::new();
    for (table, id, properties) in [
        ("places", None, vec!["we\"ird", "big", "amount", "rank"]),
        ("places_view", Some("vid"), vec!["rank"]),
    ] {
        let dataset: Dataset =
            serde_json::from_value(serde_json::json!({"id": table, "title": table,
            "source": {"schema": "features_test", "table": table, "id_column": id},
            "properties": properties}))
            .unwrap();
        let meta = inspect(&db, &config, &dataset).await.unwrap();
        sources.push((dataset, meta));
    }
    let view = sources.pop().unwrap();
    (db, sources.pop().unwrap(), view)
}

#[tokio::test]
async fn feature_pages_against_real_postgis() {
    let (db, places, view) = setup().await;
    let mut seen = Vec::new();
    for offset in [0, 3, 6] {
        let page = run(&db, &places, &format!(r#"{{"limit":3,"offset":{offset}}}"#))
            .await
            .unwrap();
        assert_eq!(page.more, offset < 6);
        seen.extend(ids(&page));
    }
    assert_eq!(seen, [1, 2, 3, 4, 5, 6, 7]);

    let asc = r#"{"sort":[{"field":"rank","direction":"asc"}]}"#;
    assert_eq!(
        ids(&run(&db, &places, asc).await.unwrap()),
        [1, 5, 2, 3, 7, 4, 6]
    );
    let mut desc = Vec::new();
    for offset in [0, 2, 4, 6] {
        let query = format!(
            r#"{{"sort":[{{"field":"rank","direction":"desc"}}],"limit":2,"offset":{offset}}}"#
        );
        desc.extend(ids(&run(&db, &places, &query).await.unwrap()));
    }
    assert_eq!(desc, [4, 6, 7, 2, 3, 1, 5]);
    let by_id = r#"{"sort":[{"field":"id","direction":"desc"}],"limit":2}"#;
    assert_eq!(ids(&run(&db, &places, by_id).await.unwrap()), [7, 6]);

    let first = run(&db, &places, r#"{"limit":1}"#).await.unwrap();
    let raw = first.features[0].get();
    assert!(raw.contains("9223372036854775807") && raw.contains("12345678901234567890.123"));
    assert!(!raw.contains("secret") && !raw.contains("\"x\""));
    let feature = &features(&first)[0];
    assert_eq!(feature["type"], "Feature");
    assert_eq!(feature["properties"]["we\"ird"], "p1");
    let lon = feature["geometry"]["coordinates"][0].as_f64().unwrap();
    assert!((lon - -102.35).abs() < 1e-6, "{feature}");

    let only = run(&db, &places, r#"{"properties":["rank"],"limit":1}"#)
        .await
        .unwrap();
    let only = &features(&only)[0];
    assert_eq!(
        (
            only["id"].as_i64(),
            only["properties"].as_object().unwrap().len()
        ),
        (Some(1), 1)
    );
    let filtered = r#"{"filter":{"op":"gte","field":"rank","value":20}}"#;
    assert_eq!(ids(&run(&db, &places, filtered).await.unwrap()), [2, 3, 7]);
    let boxed = r#"{"bbox":[-102.5,20,-100.5,21]}"#;
    assert_eq!(ids(&run(&db, &places, boxed).await.unwrap()), [1, 2]);
    let views = run(&db, &view, r#"{"limit":2,"offset":1}"#).await.unwrap();
    assert_eq!((ids(&views), views.more), (vec![20, 30], true));

    for query in [
        r#"{"limit":10001}"#,
        r#"{"limit":0}"#,
        r#"{"offset":100001}"#,
        r#"{"properties":["secret"]}"#,
        r#"{"properties":["geom"]}"#,
        r#"{"properties":["rank","rank"]}"#,
        r#"{"sort":[{"field":"secret","direction":"asc"}]}"#,
        r#"{"sort":[{"field":"geom","direction":"asc"}]}"#,
        r#"{"sort":[{"field":"rank","direction":"asc"},{"field":"rank","direction":"desc"}]}"#,
    ] {
        assert_eq!(
            run(&db, &places, query).await.err(),
            Some(Error::InvalidRequest),
            "{query}"
        );
    }
}
