use super::{connection::error, introspection::quote_identifier};
use crate::{
    domain::{Error, Metadata},
    query::predicate::{Polygon, Predicate, Spatial},
};
use sqlx::{PgConnection, Postgres, QueryBuilder};

pub fn bbox(
    builder: &mut QueryBuilder<Postgres>,
    bbox: &[f64; 4],
    meta: &Metadata,
) -> Result<(), Error> {
    if !domain(bbox[0], bbox[1]) || !domain(bbox[2], bbox[3]) {
        return Err(Error::InvalidFilter);
    }
    builder
        .push(quote_identifier(&meta.geometry_column))
        .push(" && ST_Transform(ST_MakeEnvelope(");
    for (i, value) in bbox.iter().enumerate() {
        builder.push_bind(value.to_string()).push("::float8");
        builder.push(if i < 3 { ", " } else { ", 4326), " });
    }
    builder.push(meta.srid.to_string()).push(")");
    Ok(())
}

pub fn selection(
    builder: &mut QueryBuilder<Postgres>,
    op: Spatial,
    polygons: &[Polygon],
    meta: &Metadata,
) -> Result<(), Error> {
    let geojson = geojson(polygons)?;
    let column = quote_identifier(&meta.geometry_column);
    let function = match op {
        Spatial::Intersects => "ST_Intersects",
        Spatial::Within => "ST_Within",
    };
    builder.push(&column).push(" && ");
    native(builder, &geojson, meta.srid);
    builder
        .push(" AND ")
        .push(function)
        .push("(")
        .push(&column)
        .push(", ");
    native(builder, &geojson, meta.srid);
    builder.push(")");
    Ok(())
}

pub async fn validate(
    conn: &mut PgConnection,
    predicate: &Predicate,
    meta: &Metadata,
) -> Result<(), Error> {
    let mut pending = vec![predicate];
    while let Some(node) = pending.pop() {
        match node {
            Predicate::And(args) | Predicate::Or(args) => pending.extend(args),
            Predicate::Not(arg) => pending.push(arg),
            Predicate::Spatial { polygons, .. } => {
                let valid: Option<bool> = sqlx::query_scalar(
                    "SELECT ST_IsValid(g) AND NOT ST_IsEmpty(g) AND NOT ST_IsEmpty(ST_Transform(g, $2)) FROM (SELECT ST_SetSRID(ST_GeomFromGeoJSON($1::text), 4326) AS g) AS selection",
                )
                .bind(geojson(polygons)?)
                .bind(meta.srid)
                .fetch_one(&mut *conn)
                .await
                .map_err(rejected)?;
                if valid != Some(true) {
                    return Err(Error::InvalidFilter);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn native(builder: &mut QueryBuilder<Postgres>, geojson: &str, srid: i32) {
    builder
        .push("ST_Transform(ST_SetSRID(ST_GeomFromGeoJSON(")
        .push_bind(geojson.to_owned())
        .push("::text), 4326), ")
        .push(srid.to_string())
        .push(")");
}

fn geojson(polygons: &[Polygon]) -> Result<String, Error> {
    if polygons
        .iter()
        .flatten()
        .flatten()
        .any(|[x, y]| !domain(*x, *y))
    {
        return Err(Error::InvalidFilter);
    }
    serde_json::to_string(&serde_json::json!({"type": "MultiPolygon", "coordinates": polygons}))
        .map_err(|_| Error::InvalidFilter)
}

fn rejected(failure: sqlx::Error) -> Error {
    match failure {
        sqlx::Error::Database(ref database)
            if database
                .code()
                .is_some_and(|code| code.starts_with("22") || code == "XX000") =>
        {
            Error::InvalidFilter
        }
        other => error(other),
    }
}

fn domain(x: f64, y: f64) -> bool {
    (-180.0..=180.0).contains(&x) && (-90.0..=90.0).contains(&y)
}
