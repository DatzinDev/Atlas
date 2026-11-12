use super::predicates::query_error;
use crate::{
    domain::Error,
    query::predicate::{Polygon, Spatial},
};
use sqlx::{Postgres, QueryBuilder, Transaction};

/// `None` selects no rows: the selection lies wholly outside the native CRS domain.
pub struct Geometry(Option<Vec<u8>>);

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
    if !crate::domain::bbox(bounds) {
        return Err(Error::InvalidRequest);
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
    // Clip to the Web Mercator domain and densify (1 degree) so edges follow the native CRS.
    // A transformed selection that is invalid, empty or without area is rejected, never trimmed.
    let row: Option<(bool, Option<Vec<u8>>)> = sqlx::query_as(
        "SELECT ST_IsEmpty(k), CASE WHEN NOT ST_IsEmpty(k) THEN ST_AsEWKB(s) END FROM (\
         SELECT k, CASE WHEN NOT ST_IsEmpty(k) THEN ST_Transform(CASE WHEN $2=4326 THEN k ELSE ST_Segmentize(k,1) END,$2) END AS s FROM (\
         SELECT CASE WHEN $2=3857 THEN ST_CollectionExtract(ST_Intersection(g,ST_MakeEnvelope(-180,-85.0511287798066,180,85.0511287798066,4326)),3) ELSE g END AS k \
         FROM (SELECT ST_SetSRID(ST_GeomFromGeoJSON($1),4326) AS g) AS i WHERE ST_IsValid(g) AND NOT ST_IsEmpty(g)) AS c) AS t \
         WHERE ST_IsEmpty(k) OR (ST_IsValid(s) AND NOT ST_IsEmpty(s) AND ST_Area(s) > 0)")
        .bind(json).bind(srid).fetch_optional(&mut **tx).await.map_err(input_error)?;
    match row {
        Some((true, _)) => Ok(Geometry(None)),
        Some((false, Some(bytes))) => Ok(Geometry(Some(bytes))),
        _ => Err(Error::InvalidFilter),
    }
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
        let Some(bytes) = &self.0 else {
            builder.push("FALSE");
            return Ok(());
        };
        let column = super::introspection::quote_identifier(column);
        builder
            .push("(")
            .push(&column)
            .push(" && ST_GeomFromEWKB(")
            .push_bind(bytes.clone())
            .push(") AND ")
            .push(match op {
                Spatial::Within => "ST_Within(",
                Spatial::Intersects => "ST_Intersects(",
            })
            .push(column)
            .push(", ST_GeomFromEWKB(")
            .push_bind(bytes.clone())
            .push(")))");
        Ok(())
    }
}
