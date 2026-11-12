use super::{introspection::quote_identifier, predicates::query_error, selection::Selection};
use crate::{
    config::Limits,
    domain::{Dataset, Direction, Error, Metadata, Query},
};
use futures_util::TryStreamExt;
use sqlx::{Postgres, QueryBuilder, Transaction};

/// A complete FeatureCollection body, built once and bounded by `limits.page_bytes`.
pub struct Page {
    pub body: String,
    pub more: bool,
}

pub async fn page(
    tx: &mut Transaction<'_, Postgres>,
    dataset: &Dataset,
    meta: &Metadata,
    selection: &Selection,
    query: &Query,
    limits: &Limits,
) -> Result<Page, Error> {
    let mut builder = select(dataset, meta, selection, query, limits)?;
    let limit = query.limit.unwrap_or(limits.default_limit);
    builder
        .push(" LIMIT ")
        .push_bind(i64::from(limit) + 1)
        .push(" OFFSET ")
        .push_bind(i64::from(query.offset));
    let budget = limits.page_bytes as usize;
    let mut body = String::from(r#"{"type":"FeatureCollection","features":["#);
    let (mut returned, mut more) = (0, false);
    let mut rows = builder.build_query_scalar::<String>().fetch(&mut **tx);
    while let Some(row) = rows.try_next().await.map_err(query_error)? {
        if returned == limit as usize {
            more = true;
            break;
        }
        if returned > 0 {
            body.push(',');
        }
        body.push_str(&row);
        returned += 1;
        if body.len() > budget {
            return Err(Error::QueryTooLarge);
        }
    }
    body.push_str(&format!(
        r#"],"numberReturned":{returned},"hasMore":{more}}}"#
    ));
    if body.len() > budget {
        return Err(Error::QueryTooLarge);
    }
    Ok(Page { body, more })
}

// Keep feature encoding, field validation and ordering identical for pages and cursors.
pub(crate) fn select(
    dataset: &Dataset,
    meta: &Metadata,
    selection: &Selection,
    query: &Query,
    limits: &Limits,
) -> Result<QueryBuilder<Postgres>, Error> {
    query.validate(limits.max_limit, limits.max_offset)?;
    let published: Vec<&str> = meta.attributes.iter().map(|a| a.name.as_str()).collect();
    let properties = match &query.properties {
        Some(names) => names.iter().map(String::as_str).collect(),
        None => published.clone(),
    };
    if properties.iter().any(|name| !published.contains(name)) {
        return Err(Error::InvalidRequest);
    }
    let geometry = quote_identifier(&meta.geometry_column);
    let mut builder =
        QueryBuilder::<Postgres>::new("SELECT json_build_object('type', 'Feature', 'id', ");
    builder
        .push(quote_identifier(&meta.id_column))
        .push(", 'geometry', ST_AsGeoJSON(");
    if meta.srid == 4326 {
        builder.push(&geometry);
    } else {
        builder
            .push("ST_Transform(")
            .push(&geometry)
            .push(", 4326)");
    }
    let columns: Vec<_> = properties
        .iter()
        .map(|name| quote_identifier(name))
        .collect();
    builder
        .push(")::json, 'properties', (SELECT row_to_json(p.*) FROM (SELECT ")
        .push(columns.join(", "))
        .push(") p))::text FROM ")
        .push(quote_identifier(&dataset.source.schema))
        .push(".")
        .push(quote_identifier(&dataset.source.table))
        .push(" WHERE ");
    selection.append(&mut builder)?;
    let mut order = Vec::new();
    for field in query.sort.iter().flatten() {
        if field.field != meta.id_column && !published.contains(&field.field.as_str()) {
            return Err(Error::InvalidRequest);
        }
        let direction = match field.direction {
            Direction::Asc => "ASC",
            Direction::Desc => "DESC",
        };
        order.push((field.field.as_str(), direction));
    }
    if !order.iter().any(|(name, _)| *name == meta.id_column) {
        order.push((&meta.id_column, "ASC"));
    }
    let order: Vec<_> = order
        .into_iter()
        .map(|(name, direction)| format!("{} {direction}", quote_identifier(name)))
        .collect();
    builder.push(" ORDER BY ").push(order.join(", "));
    Ok(builder)
}
