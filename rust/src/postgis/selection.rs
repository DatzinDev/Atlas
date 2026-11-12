use super::{
    predicates::compile_using,
    spatial::{self, Geometry},
};
use crate::{
    config::Limits,
    domain::{Dataset, Error, Metadata},
    query::predicate::{Predicate, Spatial},
};
use sqlx::{Postgres, QueryBuilder, Transaction};

pub struct Selection {
    predicates: Vec<Predicate>,
    geometries: Vec<Geometry>,
    bbox: Option<Geometry>,
    meta: Metadata,
}

/// Parses the admin fixed filter; call once per request, where it is used.
pub fn fixed(dataset: &Dataset, limits: &Limits) -> Result<Option<Predicate>, Error> {
    let text = dataset.filter.as_ref().map(|filter| filter.get());
    text.map(|text| crate::query::predicate::parse(text, limits))
        .transpose()
}

/// Call after authorization, in the same read transaction used to fetch source rows.
/// Takes predicates already parsed: `fixed` from [`fixed`], `user` from the API layer.
pub async fn prepare(
    tx: &mut Transaction<'_, Postgres>,
    fixed: Option<Predicate>,
    user: Option<Predicate>,
    bbox: Option<[f64; 4]>,
    meta: &Metadata,
) -> Result<Selection, Error> {
    let has_fixed = fixed.is_some();
    let predicates: Vec<Predicate> = [fixed, user].into_iter().flatten().collect();
    // Reject unpublished attributes before even validating geometries in PostGIS.
    let mut wide = meta.clone();
    wide.attributes.extend_from_slice(&meta.filter_attributes);
    for (i, predicate) in predicates.iter().enumerate() {
        // Only the admin fixed filter may use unpublished columns.
        let meta = if i == 0 && has_fixed { &wide } else { meta };
        compile_using(&mut QueryBuilder::new(""), predicate, meta, &mut |_, _| {
            Ok(())
        })?;
    }
    let mut geometries = Vec::new();
    let mut pending: Vec<_> = predicates.iter().rev().collect();
    while let Some(predicate) = pending.pop() {
        match predicate {
            Predicate::And(args) | Predicate::Or(args) => pending.extend(args.iter().rev()),
            Predicate::Not(arg) => pending.push(arg),
            Predicate::Spatial { polygons, .. } => {
                geometries.push(spatial::prepare(tx, polygons, meta.srid).await?)
            }
            _ => {}
        }
    }
    // Every bbox failure is a request error; filter geometries keep invalid_filter.
    let bbox = match bbox {
        Some(bounds) => Some(
            spatial::prepare(tx, &spatial::bbox(bounds)?, meta.srid)
                .await
                .map_err(|e| match e {
                    Error::InvalidFilter => Error::InvalidRequest,
                    other => other,
                })?,
        ),
        None => None,
    };
    Ok(Selection {
        predicates,
        geometries,
        bbox,
        meta: wide,
    })
}

impl Selection {
    pub fn append(&self, builder: &mut QueryBuilder<Postgres>) -> Result<(), Error> {
        let mut geometries = self.geometries.iter();
        builder.push("(");
        for (i, predicate) in self.predicates.iter().enumerate() {
            if i > 0 {
                builder.push(" AND ");
            }
            compile_using(builder, predicate, &self.meta, &mut |builder, op| {
                geometries.next().ok_or(Error::InternalError)?.append(
                    builder,
                    &self.meta.geometry_column,
                    op,
                )
            })?;
        }
        if self.predicates.is_empty() {
            builder.push("TRUE");
        }
        if let Some(bbox) = &self.bbox {
            builder.push(" AND ");
            bbox.append(builder, &self.meta.geometry_column, Spatial::Intersects)?;
        }
        builder.push(")");
        Ok(())
    }
}
