use super::{introspection::quote_identifier, predicates::query_error, selection::Selection};
use crate::{
    config::Limits,
    domain::{Dataset, Error, Metadata},
};
use sqlx::{Postgres, QueryBuilder, Transaction};

const HALF_WORLD: f64 = 20037508.342789244;

pub fn bounds(z: u32, x: u32, y: u32, max_zoom: u32) -> Result<[f64; 4], Error> {
    if z > max_zoom || u64::from(x) >= 1 << z || u64::from(y) >= 1 << z {
        return Err(Error::InvalidRequest);
    }
    let size = 2.0 * HALF_WORLD / f64::from(1u32 << z);
    let (minx, maxy) = (
        -HALF_WORLD + f64::from(x) * size,
        HALF_WORLD - f64::from(y) * size,
    );
    Ok([minx, maxy - size, minx + size, maxy])
}

pub async fn tile(
    tx: &mut Transaction<'_, Postgres>,
    dataset: &Dataset,
    meta: &Metadata,
    selection: &Selection,
    (z, x, y): (u32, u32, u32),
    properties: Option<&[String]>,
    limits: &Limits,
) -> Result<Vec<u8>, Error> {
    let bounds = bounds(z, x, y, limits.tile_zoom)?;
    let published: Vec<&str> = meta.attributes.iter().map(|a| a.name.as_str()).collect();
    let names: Vec<&str> = match properties {
        Some(names) => names.iter().map(String::as_str).collect(),
        None => published.clone(),
    };
    let unique: std::collections::HashSet<_> = names.iter().collect();
    if unique.len() != names.len() || names.iter().any(|name| !published.contains(name)) {
        return Err(Error::InvalidRequest);
    }
    let mut geom = String::from("__atlas_geom");
    while published.contains(&geom.as_str()) {
        geom.push('_');
    }
    let mut id = format!("{geom}_id");
    while published.contains(&id.as_str()) {
        id.push('_');
    }
    let (geom, id) = (quote_identifier(&geom), quote_identifier(&id));
    let source = quote_identifier(&meta.geometry_column);
    let key = quote_identifier(&meta.id_column);
    let mut builder = QueryBuilder::<Postgres>::new("WITH candidates AS MATERIALIZED (SELECT ");
    for attribute in meta
        .attributes
        .iter()
        .filter(|a| names.contains(&a.name.as_str()))
    {
        let column = quote_identifier(&attribute.name);
        match attribute.data_type.as_str() {
            "numeric" => builder.push(format!("{column}::text AS {column}, ")),
            // Same ISO 8601 text as the GeoJSON properties (row_to_json).
            "date" | "timestamp" | "timestamptz" => {
                builder.push(format!("to_json({column}) #>> '{{}}' AS {column}, "))
            }
            "float4" | "float8" => builder.push(format!("CASE WHEN {column}::text IN ('NaN','Infinity','-Infinity') THEN NULL ELSE {column} END AS {column}, ")),
            _ => builder.push(format!("{column}, ")),
        };
    }
    if ["int2", "int4", "int8"].contains(&meta.id_type.as_str()) {
        builder.push(format!("{source} AS {geom}, CASE WHEN {key} BETWEEN 0 AND 9007199254740991 THEN {key}::bigint END AS {id} FROM "));
    } else {
        builder.push(format!("{source} AS {geom}, NULL::bigint AS {id} FROM "));
    }
    builder
        .push(quote_identifier(&dataset.source.schema))
        .push(".")
        .push(quote_identifier(&dataset.source.table))
        .push(" WHERE ");
    let margin =
        (bounds[2] - bounds[0]) * f64::from(limits.tile_buffer) / f64::from(limits.tile_extent);
    let expanded = [
        (bounds[0] - margin).max(-HALF_WORLD),
        (bounds[1] - margin).max(-HALF_WORLD),
        (bounds[2] + margin).min(HALF_WORLD),
        (bounds[3] + margin).min(HALF_WORLD),
    ];
    if z != 0 || meta.srid == 4326 || meta.srid == 3857 {
        // Densify the envelope so its edges stay faithful in curved native projections (UTM).
        let densify = meta.srid != 4326 && meta.srid != 3857;
        builder.push(&source).push(" && ST_Transform(");
        if densify {
            builder.push("ST_Segmentize(");
        }
        builder.push("ST_MakeEnvelope(");
        for value in expanded {
            builder.push_bind(value).push(", ");
        }
        builder.push("3857)");
        if densify {
            builder
                .push(", ")
                .push_bind((expanded[2] - expanded[0]) / 16.0)
                .push(")");
        }
        builder
            .push(", ")
            .push(meta.srid.to_string())
            .push(") AND ");
    }
    builder.push(format!(
        "{source} IS NOT NULL AND NOT ST_IsEmpty({source}) AND "
    ));
    selection.append(&mut builder)?;
    builder
        .push(" LIMIT ")
        .push_bind(i64::from(limits.tile_candidates) + 1)
        .push("), total AS (SELECT count(*) AS count FROM candidates), result AS MATERIALIZED (SELECT count, CASE WHEN count <= ")
        .push_bind(i64::from(limits.tile_candidates))
        .push(" THEN (SELECT COALESCE(ST_AsMVT(encoded, ")
        .push_bind(dataset.id.clone())
        .push(", ")
        .push_bind(limits.tile_extent as i32)
        .push(", ")
        .push_bind(geom.trim_matches('"').to_owned())
        .push(", ")
        .push_bind(id.trim_matches('"').to_owned())
        .push("), ''::bytea) FROM (SELECT ");
    for name in &names {
        builder.push(quote_identifier(name)).push(", ");
    }
    builder.push(format!("{id}, ST_AsMVTGeom("));
    if meta.srid == 3857 {
        builder.push(format!("ST_Transform({geom}, 3857)"));
    } else {
        // Clip in 4326 to the buffered tile (at most ±85.0511°) so ST_Transform to 3857
        // never sees latitudes outside the Mercator domain.
        builder.push("ST_Transform(ST_ClipByBox2D(");
        if meta.srid == 4326 {
            builder.push(&geom);
        } else {
            builder.push(format!("ST_Transform({geom}, 4326)"));
        }
        builder.push(", ST_Transform(ST_MakeEnvelope(");
        for value in expanded {
            builder.push_bind(value).push(", ");
        }
        builder.push("3857), 4326)::box2d), 3857)");
    }
    builder.push(", ST_MakeEnvelope(");
    for value in bounds {
        builder.push_bind(value).push(", ");
    }
    builder
        .push("3857), ")
        .push_bind(limits.tile_extent as i32)
        .push(", ")
        .push_bind(limits.tile_buffer as i32)
        .push(format!(", true) AS {geom} FROM candidates) encoded WHERE {geom} IS NOT NULL) ELSE ''::bytea END AS tile FROM total) SELECT count, octet_length(tile) AS size, CASE WHEN octet_length(tile) <= "))
        .push_bind(i64::from(limits.tile_bytes))
        .push(" THEN tile END AS tile FROM result");
    let (count, size, tile): (i64, i32, Option<Vec<u8>>) = builder
        .build_query_as()
        .fetch_one(&mut **tx)
        .await
        .map_err(query_error)?;
    if count > i64::from(limits.tile_candidates) || i64::from(size) > i64::from(limits.tile_bytes) {
        return Err(Error::QueryTooLarge);
    }
    tile.ok_or(Error::InternalError)
}
