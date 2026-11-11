use crate::{
    catalog::Publication,
    config::Limits,
    domain::{Error, Query},
    postgis::{
        connection::Database, features, introspection::quote_identifier, predicates::query_error,
        selection::prepare,
    },
};
use sqlx::{Execute, Postgres, QueryBuilder, Transaction};
use std::{collections::VecDeque, time::Duration};
use tokio::{
    sync::OwnedSemaphorePermit,
    time::{Instant, timeout_at},
};

const END: &str = "]}";

pub struct Export {
    tx: Option<Transaction<'static, Postgres>>,
    permit: Option<OwnedSemaphorePermit>,
    pending: VecDeque<String>,
    deadline: Instant,
    remaining: usize,
    first: bool,
    started: bool,
    done: bool,
}

impl Export {
    pub async fn open(
        db: &Database,
        publication: &Publication,
        query: &Query,
        limits: &Limits,
    ) -> Result<Self, Error> {
        Self::admitted(db, publication, query, limits, None).await
    }

    pub(crate) async fn admitted(
        db: &Database,
        publication: &Publication,
        query: &Query,
        limits: &Limits,
        permit: Option<OwnedSemaphorePermit>,
    ) -> Result<Self, Error> {
        if query.limit.is_some() || query.offset != 0 || query.sort.is_some() {
            return Err(Error::InvalidRequest);
        }
        let remaining = (limits.export_bytes as usize)
            .checked_sub(END.len())
            .ok_or(Error::QueryTooLarge)?;
        let mut export = Self {
            tx: Some(db.begin_read().await?),
            permit,
            pending: VecDeque::new(),
            deadline: Instant::now() + Duration::from_secs(limits.stream_seconds.into()),
            remaining,
            first: true,
            started: false,
            done: false,
        };
        let deadline = export.deadline;
        timeout_at(deadline, async {
            let tx = export.tx.as_mut().ok_or(Error::InternalError)?;
            let (dataset, meta) = (&publication.config, &publication.metadata);
            let selection = prepare(
                tx,
                dataset.filter.as_ref().map(|f| f.get()),
                query.filter.as_ref().map(|f| f.get()),
                query.bbox,
                meta,
                limits,
            )
            .await?;
            let mut select = features::select(dataset, meta, &selection, query, limits)?;
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
            let sql = select.sql().as_str().to_owned();
            let arguments = select
                .build()
                .take_arguments()
                .map_err(|_| Error::InternalError)?;
            cursor.push(sql);
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
        let rows: Vec<String> = sqlx::query_scalar("FETCH FORWARD 128 FROM atlas_export")
            .fetch_all(&mut **tx)
            .await
            .map_err(query_error)?;
        self.done = rows.len() < 128;
        self.pending.extend(rows);
        Ok(())
    }

    pub(crate) fn deadline(&self) -> Instant {
        self.deadline
    }

    pub async fn next(&mut self) -> Result<Option<String>, Error> {
        let deadline = self.deadline;
        timeout_at(deadline, self.chunk())
            .await
            .map_err(|_| Error::QueryTimeout)?
    }

    async fn chunk(&mut self) -> Result<Option<String>, Error> {
        if Instant::now() >= self.deadline {
            return Err(Error::QueryTimeout);
        }
        let chunk = if !self.started {
            self.started = true;
            r#"{"type":"FeatureCollection","features":["#.to_owned()
        } else {
            if self.pending.is_empty() && !self.done {
                self.fetch().await?;
            }
            if let Some(row) = self.pending.pop_front() {
                let comma = if self.first { "" } else { "," };
                self.first = false;
                format!("{comma}{row}")
            } else if let Some(tx) = self.tx.take() {
                let permit = self.permit.take();
                tokio::spawn(async move {
                    let _permit = permit;
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
            let permit = self.permit.take();
            tokio::spawn(async move {
                let _permit = permit;
                let _ = tx.rollback().await;
            });
        }
    }
}
