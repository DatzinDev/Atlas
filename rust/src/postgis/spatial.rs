use super::predicates::query_error;
use crate::{
    domain::Error,
    query::predicate::{Polygon, Spatial},
};
use sqlx::{Postgres, QueryBuilder, Transaction};

pub struct Geometry(Vec<u8>);

fn input_error(failure: sqlx::Error) -> Error {
    if let sqlx::Error::Database(ref database) = failure
        && database.code().as_deref() == Some("XX000")
    {
        return Error::InvalidFilter;
    }
    query_error(failure)
}

pub fn bbox(bounds: [f64; 4]) -> Result<Vec<Polygon>, Error> {
    let [x0, y0, x1, y1] = bounds;
    if bounds.iter().any(|v| !v.is_finite()) || x0 > x1 || y0 > y1 {
        return Err(Error::InvalidFilter);
    }
    Ok(vec![vec![vec![
        [x0, y0],
        [x1, y0],
        [x1, y1],
        [x0, y1],
        [x0, y0],
    ]]])
}

/// Validate and transform only the selection; never repair or transform source columns.
pub async fn prepare(
    tx: &mut Transaction<'_, Postgres>,
    polygons: &[Polygon],
    srid: i32,
) -> Result<Geometry, Error> {
    if polygons.is_empty()
        || polygons.iter().any(Vec::is_empty)
        || polygons.iter().flatten().any(|ring| {
            ring.len() < 4
                || ring.first() != ring.last()
                || ring.iter().any(|[x, y]| {
                    !x.is_finite()
                        || !y.is_finite()
                        || !(-180.0..=180.0).contains(x)
                        || !(-90.0..=90.0).contains(y)
                })
        })
    {
        return Err(Error::InvalidFilter);
    }
    let known: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM spatial_ref_sys WHERE srid=$1)")
            .bind(srid)
            .fetch_one(&mut **tx)
            .await
            .map_err(query_error)?;
    if srid <= 0 || !known {
        return Err(Error::SchemaChanged);
    }
    let json = serde_json::json!({"type":"MultiPolygon", "coordinates":polygons}).to_string();
    let bytes: Option<Vec<u8>> = sqlx::query_scalar(
        "SELECT CASE WHEN ST_IsValid(g) AND NOT ST_IsEmpty(g) THEN ST_AsEWKB(ST_Transform(g,$2)) END FROM (SELECT ST_SetSRID(ST_GeomFromGeoJSON($1),4326) AS g) AS selection")
        .bind(json).bind(srid).fetch_one(&mut **tx).await.map_err(input_error)?;
    bytes.map(Geometry).ok_or(Error::InvalidFilter)
}

impl Geometry {
    pub fn append(
        &self,
        builder: &mut QueryBuilder<Postgres>,
        column: &str,
        op: Spatial,
    ) -> Result<(), Error> {
        if !crate::domain::identifier(column) {
            return Err(Error::SchemaChanged);
        }
        let column = super::introspection::quote_identifier(column);
        builder
            .push("(")
            .push(&column)
            .push(" && ST_GeomFromEWKB(")
            .push_bind(self.0.clone())
            .push(") AND ")
            .push(match op {
                Spatial::Within => "ST_Within(",
                Spatial::Intersects => "ST_Intersects(",
            })
            .push(column)
            .push(", ST_GeomFromEWKB(")
            .push_bind(self.0.clone())
            .push(")))");
        Ok(())
    }
}
