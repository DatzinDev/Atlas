use super::{
    predicates::compile_using,
    spatial::{self, Geometry},
};
use crate::{
    config::Limits,
    domain::{Dataset, Error, Metadata},
    query::predicate::{Polygon, Predicate, Spatial},
};
use sqlx::{Postgres, QueryBuilder, Transaction};

/// Prepared mirror of a [`Predicate`]: each spatial node owns its own geometry.
enum Node {
    And(Vec<Node>),
    Or(Vec<Node>),
    Not(Box<Node>),
    Pending(Spatial, Vec<Polygon>),
    Spatial(Spatial, Geometry),
    Leaf(Predicate),
}

fn lift(predicate: Predicate) -> Node {
    match predicate {
        Predicate::And(args) => Node::And(args.into_iter().map(lift).collect()),
        Predicate::Or(args) => Node::Or(args.into_iter().map(lift).collect()),
        Predicate::Not(arg) => Node::Not(Box::new(lift(*arg))),
        Predicate::Spatial { op, polygons } => Node::Pending(op, polygons),
        leaf => Node::Leaf(leaf),
    }
}

pub struct Selection {
    nodes: Vec<Node>,
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
    let mut nodes: Vec<Node> = predicates.into_iter().map(lift).collect();
    // Prepare in filter order (pre-order, left to right), binding each geometry in place.
    let mut pending: Vec<&mut Node> = nodes.iter_mut().rev().collect();
    while let Some(node) = pending.pop() {
        match node {
            Node::And(args) | Node::Or(args) => pending.extend(args.iter_mut().rev()),
            Node::Not(arg) => pending.push(arg),
            Node::Pending(op, polygons) => {
                let op = *op;
                let geometry = spatial::prepare(tx, polygons, meta.srid).await?;
                *node = Node::Spatial(op, geometry);
            }
            Node::Spatial(..) | Node::Leaf(_) => {}
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
        nodes,
        bbox,
        meta: wide,
    })
}

impl Selection {
    pub fn append(&self, builder: &mut QueryBuilder<Postgres>) -> Result<(), Error> {
        builder.push("(");
        for (i, node) in self.nodes.iter().enumerate() {
            if i > 0 {
                builder.push(" AND ");
            }
            self.append_node(builder, node)?;
        }
        if self.nodes.is_empty() {
            builder.push("TRUE");
        }
        if let Some(bbox) = &self.bbox {
            builder.push(" AND ");
            bbox.append(builder, &self.meta.geometry_column, Spatial::Intersects)?;
        }
        builder.push(")");
        Ok(())
    }

    fn append_node(&self, builder: &mut QueryBuilder<Postgres>, node: &Node) -> Result<(), Error> {
        match node {
            Node::And(args) | Node::Or(args) => {
                let joiner = if matches!(node, Node::And(_)) {
                    " AND "
                } else {
                    " OR "
                };
                builder.push("(");
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 {
                        builder.push(joiner);
                    }
                    self.append_node(builder, arg)?;
                }
                builder.push(")");
            }
            Node::Not(arg) => {
                builder.push("(NOT ");
                self.append_node(builder, arg)?;
                builder.push(")");
            }
            Node::Spatial(op, geometry) => {
                geometry.append(builder, &self.meta.geometry_column, *op)?
            }
            Node::Pending(..) => return Err(Error::InternalError),
            // Leaves hold no spatial nodes: `lift` lifts every one out.
            Node::Leaf(predicate) => compile_using(builder, predicate, &self.meta, &mut |_, _| {
                Err(Error::InternalError)
            })?,
        }
        Ok(())
    }
}
