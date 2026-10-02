use crate::{storage_error, ServiceDatabase};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use panel_errors::{PanelError, Result};
use panel_event_codec::protobuf;
use panel_events::EventEnvelope;
use panel_outbox::{OutboxPosition, OutboxRecord, OutboxSource, OutboxWakeup};
use sqlx::{
    pool::PoolConnection,
    postgres::{PgAdvisoryLock, PgAdvisoryLockGuard, PgConnection, PgListener, PgPool},
    Either, Postgres,
};
use std::time::Duration;
use tokio::sync::Mutex;
use uuid::Uuid;

const MAX_ERROR_BYTES: usize = 1024;

/// Unpublished outbox size, for alerting on a stalled relay.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct OutboxBacklog {
    pub pending: u64,
    pub oldest_recorded_at: Option<DateTime<Utc>>,
}

/// The transactional outbox of one service schema.
#[derive(Clone, Debug)]
pub struct PgOutbox {
    pool: PgPool,
    channel: String,
    lock_key: String,
}

impl PgOutbox {
    pub fn new(database: &ServiceDatabase) -> Self {
        let schema = database.schema().as_str();
        Self {
            pool: database.pool().clone(),
            channel: format!("{schema}_outbox"),
            lock_key: format!("pingora-panel.outbox-relay.{schema}"),
        }
    }

    /// Appends an event inside the caller's transaction. The event becomes
    /// visible to the relay exactly when that transaction commits.
    pub async fn append(connection: &mut PgConnection, envelope: &EventEnvelope) -> Result<()> {
        sqlx::query(
            "INSERT INTO outbox (event_id, event_type, subject, cloudevent) VALUES ($1, $2, $3, $4)",
        )
        .bind(Uuid::from_bytes(*envelope.event_id().as_bytes()))
        .bind(envelope.qualified_type())
        .bind(envelope.aggregate().subject())
        .bind(protobuf::encode(envelope))
        .execute(connection)
        .await
        .map_err(storage_error)?;
        Ok(())
    }

    /// Subscribes to commit notifications for this outbox.
    pub async fn listen(&self) -> Result<PgOutboxWakeup> {
        let mut listener = PgListener::connect_with(&self.pool)
            .await
            .map_err(storage_error)?;
        listener
            .listen(&self.channel)
            .await
            .map_err(storage_error)?;
        Ok(PgOutboxWakeup {
            listener: Mutex::new(listener),
        })
    }

    /// Tries to become this outbox's only relay. Leadership lasts while the
    /// returned guard and its session live; a crashed leader's session ends
    /// and releases it.
    pub async fn try_lead(&self) -> Result<Option<PgRelayLeadership>> {
        let connection = self.pool.acquire().await.map_err(storage_error)?;
        match PgAdvisoryLock::new(&self.lock_key)
            .try_acquire(connection)
            .await
            .map_err(storage_error)?
        {
            Either::Left(guard) => Ok(Some(PgRelayLeadership { guard })),
            Either::Right(_) => Ok(None),
        }
    }

    /// Deletes published rows older than `older_than`, at most `limit` rows.
    pub async fn purge_published(&self, older_than: Duration, limit: i64) -> Result<u64> {
        let result = sqlx::query(
            "DELETE FROM outbox WHERE position IN (\
                 SELECT position FROM outbox \
                 WHERE published_at < now() - make_interval(secs => $1) \
                 ORDER BY position LIMIT $2)",
        )
        .bind(older_than.as_secs_f64())
        .bind(limit)
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        Ok(result.rows_affected())
    }

    pub async fn backlog(&self) -> Result<OutboxBacklog> {
        let (pending, oldest): (i64, Option<DateTime<Utc>>) = sqlx::query_as(
            "SELECT count(*), min(recorded_at) FROM outbox WHERE published_at IS NULL",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(storage_error)?;
        Ok(OutboxBacklog {
            pending: u64::try_from(pending).unwrap_or_default(),
            oldest_recorded_at: oldest,
        })
    }
}

#[async_trait]
impl OutboxSource for PgOutbox {
    async fn pending(&self, limit: usize) -> Result<Vec<OutboxRecord>> {
        let rows: Vec<(i64, Vec<u8>, i32)> = sqlx::query_as(
            "SELECT position, cloudevent, attempts FROM outbox \
             WHERE published_at IS NULL ORDER BY position LIMIT $1",
        )
        .bind(i64::try_from(limit).unwrap_or(i64::MAX))
        .fetch_all(&self.pool)
        .await
        .map_err(storage_error)?;
        rows.into_iter()
            .map(|(position, cloudevent, attempts)| {
                let envelope = protobuf::decode(&cloudevent).map_err(|error| {
                    PanelError::corrupt_state(format!(
                        "outbox record {position} is not a valid CloudEvent: {}",
                        error.message
                    ))
                })?;
                Ok(OutboxRecord::new(
                    OutboxPosition::new(u64::try_from(position).unwrap_or_default()),
                    envelope,
                    u32::try_from(attempts).unwrap_or_default(),
                ))
            })
            .collect()
    }

    async fn mark_published(&self, positions: &[OutboxPosition]) -> Result<()> {
        let positions = positions
            .iter()
            .map(|position| i64::try_from(position.get()).unwrap_or(i64::MAX))
            .collect::<Vec<_>>();
        sqlx::query(
            "UPDATE outbox SET published_at = now() \
             WHERE position = ANY($1) AND published_at IS NULL",
        )
        .bind(positions)
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        Ok(())
    }

    async fn record_failure(&self, position: OutboxPosition, reason: &str) -> Result<()> {
        let reason = reason
            .char_indices()
            .take_while(|(index, character)| index + character.len_utf8() <= MAX_ERROR_BYTES)
            .map(|(_, character)| character)
            .collect::<String>();
        sqlx::query(
            "UPDATE outbox SET attempts = attempts + 1, last_error = $2, last_attempt_at = now() \
             WHERE position = $1",
        )
        .bind(i64::try_from(position.get()).unwrap_or(i64::MAX))
        .bind(reason)
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        Ok(())
    }
}

/// Commit notifications for one outbox.
pub struct PgOutboxWakeup {
    listener: Mutex<PgListener>,
}

#[async_trait]
impl OutboxWakeup for PgOutboxWakeup {
    async fn wait(&self, timeout: Duration) {
        let mut listener = self.listener.lock().await;
        // A lost connection drops notifications; the timeout bounds how long
        // the relay can miss committed rows.
        match tokio::time::timeout(timeout, listener.recv()).await {
            Ok(Ok(_)) => while listener.next_buffered().is_some() {},
            Ok(Err(_)) => {
                drop(listener);
                tokio::time::sleep(timeout).await;
            }
            Err(_) => {}
        }
    }
}

/// Exclusive relay leadership for one outbox. Dropping it releases the lock
/// when its connection is next used; [`PgRelayLeadership::release`] hands
/// leadership over immediately.
pub struct PgRelayLeadership {
    guard: PgAdvisoryLockGuard<PoolConnection<Postgres>>,
}

impl PgRelayLeadership {
    pub async fn release(self) -> Result<()> {
        self.guard
            .release_now()
            .await
            .map(drop)
            .map_err(storage_error)
    }
}
