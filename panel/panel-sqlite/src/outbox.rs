use crate::{storage_error, ServiceDatabase};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use panel_errors::{PanelError, Result};
use panel_event_codec::protobuf;
use panel_events::EventEnvelope;
use panel_outbox::{OutboxPosition, OutboxRecord, OutboxSource, OutboxWakeup};
use sqlx::sqlite::{SqliteConnection, SqlitePool};
use std::{sync::Arc, time::Duration};
use tokio::sync::Notify;
use uuid::Uuid;

const MAX_ERROR_BYTES: usize = 1024;
/// How often the relay looks for events appended by transactions that did
/// not wake it.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Unpublished outbox size, for alerting on a stalled relay.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct OutboxBacklog {
    pub pending: u64,
    pub oldest_recorded_at: Option<DateTime<Utc>>,
}

/// The transactional outbox of one module's database. One process relays
/// it, so it needs no leadership.
#[derive(Clone, Debug)]
pub struct SqliteOutbox {
    pool: SqlitePool,
    commits: Arc<Notify>,
}

impl SqliteOutbox {
    pub fn new(database: &ServiceDatabase) -> Self {
        Self {
            pool: database.pool().clone(),
            commits: database.commits(),
        }
    }

    /// Appends an event inside the caller's transaction. The event becomes
    /// visible to the relay exactly when that transaction commits.
    pub async fn append(connection: &mut SqliteConnection, envelope: &EventEnvelope) -> Result<()> {
        sqlx::query(
            "INSERT INTO outbox (event_id, event_type, subject, recorded_at, cloudevent) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(Uuid::from_bytes(*envelope.event_id().as_bytes()))
        .bind(envelope.qualified_type())
        .bind(envelope.aggregate().subject())
        .bind(Utc::now())
        .bind(protobuf::encode(envelope))
        .execute(connection)
        .await
        .map_err(storage_error)?;
        Ok(())
    }

    /// What wakes the relay: commits the module reports, or the poll.
    pub fn wakeup(&self) -> SqliteOutboxWakeup {
        SqliteOutboxWakeup {
            commits: self.commits.clone(),
        }
    }

    /// Deletes published rows older than `older_than`, at most `limit` rows.
    pub async fn purge_published(&self, older_than: Duration, limit: i64) -> Result<u64> {
        let result = sqlx::query(
            "DELETE FROM outbox WHERE position IN (\
                 SELECT position FROM outbox WHERE published_at < ?1 \
                 ORDER BY position LIMIT ?2)",
        )
        .bind(cutoff(older_than))
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

/// The time `age` ago.
pub(crate) fn cutoff(age: Duration) -> DateTime<Utc> {
    Utc::now() - crate::time::span(age)
}

#[async_trait]
impl OutboxSource for SqliteOutbox {
    async fn pending(&self, limit: usize) -> Result<Vec<OutboxRecord>> {
        let rows: Vec<(i64, Vec<u8>, i64)> = sqlx::query_as(
            "SELECT position, cloudevent, attempts FROM outbox \
             WHERE published_at IS NULL ORDER BY position LIMIT ?1",
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
            .map(|position| position.get().to_string())
            .collect::<Vec<_>>()
            .join(",");
        sqlx::query(
            "UPDATE outbox SET published_at = ?1 \
             WHERE position IN (SELECT value FROM json_each(?2)) AND published_at IS NULL",
        )
        .bind(Utc::now())
        .bind(format!("[{positions}]"))
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
            "UPDATE outbox SET attempts = attempts + 1, last_error = ?2, last_attempt_at = ?3 \
             WHERE position = ?1",
        )
        .bind(i64::try_from(position.get()).unwrap_or(i64::MAX))
        .bind(reason)
        .bind(Utc::now())
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        Ok(())
    }
}

/// Wakes the relay when the module reports a commit, and otherwise every
/// 250 milliseconds.
pub struct SqliteOutboxWakeup {
    commits: Arc<Notify>,
}

#[async_trait]
impl OutboxWakeup for SqliteOutboxWakeup {
    async fn wait(&self, timeout: Duration) {
        let _ = tokio::time::timeout(timeout.min(POLL_INTERVAL), self.commits.notified()).await;
    }
}
