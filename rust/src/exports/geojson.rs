use crate::{
    catalog::Publication,
    config::Limits,
    domain::{Error, Query},
    postgis::{
        connection::Database,
        features,
        introspection::quote_identifier,
        predicates::query_error,
        selection::{fixed, prepare},
    },
    query::predicate::Predicate,
};
use futures_util::TryStreamExt;
use sqlx::{Execute, Postgres, QueryBuilder, Transaction};
use std::{collections::VecDeque, time::Duration};
use tokio::{
    sync::OwnedSemaphorePermit,
    time::{Instant, timeout_at},
};

const HEADER: &str = r#"{"type":"FeatureCollection","features":["#;
const END: &str = "]}";

pub struct Export {
    tx: Option<Transaction<'static, Postgres>>,
    permits: Vec<OwnedSemaphorePermit>,
    pending: VecDeque<String>,
    deadline: Instant,
    remaining: usize,
    first: bool,
    started: bool,
    done: bool,
    overflow: bool,
}

impl Export {
    /// Only `Publications::export` opens exports, holding its admission `permits`.
    /// `user` is `query.filter` already parsed by the caller.
    pub(crate) async fn admitted(
        db: &Database,
        publication: &Publication,
        query: &Query,
        user: Option<Predicate>,
        limits: &Limits,
        permits: Vec<OwnedSemaphorePermit>,
    ) -> Result<Self, Error> {
        if query.limit.is_some() || query.offset != 0 || query.sort.is_some() {
            return Err(Error::InvalidRequest);
        }
        let remaining = (limits.export_bytes as usize)
            .checked_sub(END.len())
            .ok_or(Error::QueryTooLarge)?;
        let mut export = Self {
            tx: Some(db.begin_data().await?),
            permits,
            pending: VecDeque::new(),
            deadline: Instant::now() + Duration::from_secs(limits.stream_seconds.into()),
            remaining,
            first: true,
            started: false,
            done: false,
            overflow: false,
        };
        let deadline = export.deadline;
        timeout_at(deadline, async {
            let tx = export.tx.as_mut().ok_or(Error::InternalError)?;
            let (dataset, meta) = (&publication.config, &publication.metadata);
            let selection = prepare(tx, fixed(dataset, limits)?, user, query.bbox, meta).await?;
            let mut select = features::select(dataset, meta, &selection, query)?;
            let mut count = QueryBuilder::<Postgres>::new("SELECT count(*) FROM (SELECT 1 FROM ");
            count
                .push(quote_identifier(&dataset.source.schema))
                .push(".")
                .push(quote_identifier(&dataset.source.table))
                .push(" WHERE ");
            selection.append(&mut count)?;
            count
                .push(" LIMIT ")
                .push_bind(i64::from(limits.export_features) + 1)
                .push(") candidates");
            let total: i64 = count
                .build_query_scalar()
                .fetch_one(&mut **tx)
                .await
                .map_err(query_error)?;
            if total > i64::from(limits.export_features) {
                return Err(Error::QueryTooLarge);
            }
            // QueryBuilder keeps bind values separate even inside DECLARE.
            let mut cursor =
                QueryBuilder::<Postgres>::new("DECLARE atlas_export NO SCROLL CURSOR FOR ");
            // Oversized features come back as NULL so they never cross the wire.
            let inner = select.sql().as_str().to_owned();
            select.push_bind(i64::from(limits.export_bytes));
            let bound = select.sql().as_str()[inner.len()..].to_owned();
            let arguments = select
                .build()
                .take_arguments()
                .map_err(|_| Error::InternalError)?;
            cursor
                .push("SELECT CASE WHEN octet_length(f) <= ")
                .push(bound)
                .push(" THEN f END FROM (")
                .push(inner)
                .push(") atlas_rows(f)");
            sqlx::query_with(cursor.sql(), arguments.ok_or(Error::InternalError)?)
                .execute(&mut **tx)
                .await
                .map_err(query_error)?;
            export.fetch().await?;
            Ok::<_, Error>(())
        })
        .await
        .map_err(|_| Error::QueryTimeout)??;
        Ok(export)
    }

    async fn fetch(&mut self) -> Result<(), Error> {
        let tx = self.tx.as_mut().ok_or(Error::InternalError)?;
        // Called with `pending` empty: buffer rows one by one only while they fit the budget.
        let mut budget = self
            .remaining
            .saturating_sub(if self.started { 0 } else { HEADER.len() });
        let mut rows =
            sqlx::query_scalar::<_, Option<String>>("FETCH FORWARD 128 FROM atlas_export")
                .fetch(&mut **tx);
        let mut fetched = 0;
        while let Some(row) = rows.try_next().await.map_err(query_error)? {
            fetched += 1;
            let cost = usize::from(!(self.first && self.pending.is_empty()));
            match row.filter(|row| row.len() + cost <= budget) {
                Some(row) => {
                    budget -= row.len() + cost;
                    self.pending.push_back(row);
                }
                None => {
                    self.overflow = true;
                    break;
                }
            }
        }
        self.done = self.overflow || fetched < 128;
        Ok(())
    }

    pub(crate) fn deadline(&self) -> Instant {
        self.deadline
    }

    /// No deadline here: `api/export.rs` wraps its whole pump loop in one `timeout_at(deadline())`.
    pub async fn next(&mut self) -> Result<Option<String>, Error> {
        let chunk = if !self.started {
            self.started = true;
            HEADER.to_owned()
        } else {
            if self.pending.is_empty() && !self.done {
                self.fetch().await?;
            }
            if let Some(row) = self.pending.pop_front() {
                let comma = if self.first { "" } else { "," };
                self.first = false;
                format!("{comma}{row}")
            } else if self.overflow {
                return Err(Error::QueryTooLarge);
            } else if let Some(tx) = self.tx.take() {
                let permits = std::mem::take(&mut self.permits);
                tokio::spawn(async move {
                    let _permits = permits;
                    tx.rollback()
                        .await
                        .map_err(crate::postgis::connection::error)
                })
                .await
                .map_err(|_| Error::InternalError)??;
                return Ok(Some(END.to_owned()));
            } else {
                return Ok(None);
            }
        };
        self.remaining = self
            .remaining
            .checked_sub(chunk.len())
            .ok_or(Error::QueryTooLarge)?;
        Ok(Some(chunk))
    }
}

impl Drop for Export {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            // Keep the connection leased until rollback has really completed.
            let permits = std::mem::take(&mut self.permits);
            tokio::spawn(async move {
                let _permits = permits;
                let _ = tx.rollback().await;
            });
        }
    }
}
