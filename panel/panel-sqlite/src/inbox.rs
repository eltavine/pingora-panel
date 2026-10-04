use crate::{outbox::cutoff, storage_error, ServiceDatabase};
use async_trait::async_trait;
use chrono::Utc;
use panel_errors::Result;
use panel_events::{ConsumerName, EventId, InboxClaim, ProcessedEventStore};
use sqlx::sqlite::{SqliteConnection, SqlitePool};
use std::time::Duration;
use uuid::Uuid;

/// Durable record of the events each consumer of a module has processed.
///
/// Two ways to use it, following the idempotent consumer pattern:
/// - handlers whose effects live in this database call
///   [`SqliteProcessedEventStore::record_in`] inside their own transaction,
///   so the effect and the record commit or roll back together;
/// - other handlers are wrapped in `IdempotentEventHandler`, which claims an
///   event before running the handler and completes the claim afterwards.
#[derive(Clone, Debug)]
pub struct SqliteProcessedEventStore {
    pool: SqlitePool,
}

impl SqliteProcessedEventStore {
    pub fn new(database: &ServiceDatabase) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    /// Records the event as processed inside the caller's transaction.
    /// Returns `false` when the consumer already processed it, in which case
    /// the caller should skip its effects.
    pub async fn record_in(
        connection: &mut SqliteConnection,
        consumer: &ConsumerName,
        event_id: EventId,
    ) -> Result<bool> {
        let result = sqlx::query(
            "INSERT INTO processed_events (consumer, event_id, processed_at) \
             VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING",
        )
        .bind(consumer.as_str())
        .bind(uuid(event_id))
        .bind(Utc::now())
        .execute(connection)
        .await
        .map_err(storage_error)?;
        Ok(result.rows_affected() == 1)
    }

    /// Deletes completions older than `older_than`, at most `limit` rows.
    /// Retention must exceed how long the broker can redeliver an event.
    pub async fn purge_processed(&self, older_than: Duration, limit: i64) -> Result<u64> {
        let result = sqlx::query(
            "DELETE FROM processed_events WHERE (consumer, event_id) IN (\
                 SELECT consumer, event_id FROM processed_events \
                 WHERE processed_at < ?1 LIMIT ?2)",
        )
        .bind(cutoff(older_than))
        .bind(limit)
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        Ok(result.rows_affected())
    }
}

fn uuid(event_id: EventId) -> Uuid {
    Uuid::from_bytes(*event_id.as_bytes())
}

#[async_trait]
impl ProcessedEventStore for SqliteProcessedEventStore {
    async fn claim(
        &self,
        consumer: &ConsumerName,
        event_id: EventId,
        lease: Duration,
    ) -> Result<InboxClaim> {
        // Insert a claim, or take over one whose lease lapsed. The statement
        // holds the file's write lock, so concurrent claims are serialized.
        let now = Utc::now();
        let acquired = sqlx::query_scalar::<_, i64>(
            "INSERT INTO processed_events (consumer, event_id, claimed_until) \
             VALUES (?1, ?2, ?3) \
             ON CONFLICT (consumer, event_id) DO UPDATE \
             SET claimed_until = excluded.claimed_until \
             WHERE processed_events.processed_at IS NULL \
               AND processed_events.claimed_until <= ?4 \
             RETURNING 1",
        )
        .bind(consumer.as_str())
        .bind(uuid(event_id))
        .bind(now + crate::time::span(lease))
        .bind(now)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?;
        if acquired.is_some() {
            return Ok(InboxClaim::Acquired);
        }
        let processed = sqlx::query_scalar::<_, bool>(
            "SELECT processed_at IS NOT NULL FROM processed_events \
             WHERE consumer = ?1 AND event_id = ?2",
        )
        .bind(consumer.as_str())
        .bind(uuid(event_id))
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?;
        Ok(match processed {
            Some(true) => InboxClaim::AlreadyProcessed,
            // A claim released between the two statements is retried later.
            Some(false) | None => InboxClaim::InFlight,
        })
    }

    async fn complete(&self, consumer: &ConsumerName, event_id: EventId) -> Result<()> {
        sqlx::query(
            "UPDATE processed_events SET processed_at = ?3, claimed_until = NULL \
             WHERE consumer = ?1 AND event_id = ?2 AND processed_at IS NULL",
        )
        .bind(consumer.as_str())
        .bind(uuid(event_id))
        .bind(Utc::now())
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        Ok(())
    }

    async fn release(&self, consumer: &ConsumerName, event_id: EventId) -> Result<()> {
        sqlx::query(
            "DELETE FROM processed_events \
             WHERE consumer = ?1 AND event_id = ?2 AND processed_at IS NULL",
        )
        .bind(consumer.as_str())
        .bind(uuid(event_id))
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        Ok(())
    }
}
