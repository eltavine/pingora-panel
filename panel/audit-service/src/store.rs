//! Audit records in PostgreSQL: appended once per event under the chain
//! head's lock, listed by filter, and verified against their hashes and
//! checkpoints.

use crate::chain::Entry;
use chrono::{DateTime, Utc};
use panel_errors::{PanelError, Result};
use panel_events::EventEnvelope;
use panel_postgres::ServiceDatabase;
use sqlx::{PgPool, Row};

/// A checkpoint of the head is written every this many records.
const CHECKPOINT_EVERY: u64 = 256;
/// Records verified per query.
const VERIFY_BATCH: i64 = 1_000;

macro_rules! columns {
    () => {
        "sequence, event_id, source, event_type, event_version, subject, occurred_at, \
         recorded_at, actor_type, actor_id, correlation_id, causation_id, idempotency_key, \
         traceparent, data, previous_hash, hash"
    };
}

/// One stored record with its chain links.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Record {
    pub entry: Entry,
    pub previous_hash: String,
    pub hash: String,
}

/// What to list; empty strings and `None` match everything.
#[derive(Clone, Debug, Default)]
pub struct Filter {
    pub before: Option<u64>,
    pub limit: u32,
    pub actor_id: String,
    /// An exact event type, or a prefix ending in `.`.
    pub event_type: String,
    pub subject: String,
    pub correlation_id: String,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
}

/// The outcome of verifying part of the chain.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Verification {
    pub checked: u64,
    pub first_mismatch: Option<u64>,
    pub head_sequence: u64,
    pub head_hash: String,
}

#[derive(Clone)]
pub struct PgAuditStore {
    pool: PgPool,
}

fn storage(error: sqlx::Error) -> PanelError {
    PanelError::storage_unavailable("audit storage failed").with_source(error)
}

fn sequence(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| PanelError::invalid_argument("sequence is out of range"))
}

fn record(row: &sqlx::postgres::PgRow) -> Result<Record> {
    let get = |error: sqlx::Error| storage(error);
    let number: i64 = row.try_get("sequence").map_err(get)?;
    let version: i32 = row.try_get("event_version").map_err(get)?;
    Ok(Record {
        entry: Entry {
            sequence: u64::try_from(number).unwrap_or_default(),
            event_id: row.try_get("event_id").map_err(get)?,
            source: row.try_get("source").map_err(get)?,
            event_type: row.try_get("event_type").map_err(get)?,
            event_version: u32::try_from(version).unwrap_or_default(),
            subject: row.try_get("subject").map_err(get)?,
            occurred_at: row.try_get("occurred_at").map_err(get)?,
            recorded_at: row.try_get("recorded_at").map_err(get)?,
            actor_type: row.try_get("actor_type").map_err(get)?,
            actor_id: row.try_get("actor_id").map_err(get)?,
            correlation_id: row.try_get("correlation_id").map_err(get)?,
            causation_id: row.try_get("causation_id").map_err(get)?,
            idempotency_key: row.try_get("idempotency_key").map_err(get)?,
            traceparent: row.try_get("traceparent").map_err(get)?,
            data: row.try_get("data").map_err(get)?,
        },
        previous_hash: row.try_get("previous_hash").map_err(get)?,
        hash: row.try_get("hash").map_err(get)?,
    })
}

impl PgAuditStore {
    pub fn new(database: &ServiceDatabase) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    /// Appends `event` unless it is already recorded; returns its sequence.
    pub async fn append(&self, event: &EventEnvelope) -> Result<u64> {
        let mut transaction = self.pool.begin().await.map_err(storage)?;
        let head = sqlx::query("SELECT sequence, hash FROM audit_head FOR UPDATE")
            .fetch_one(&mut *transaction)
            .await
            .map_err(storage)?;
        let existing: Option<i64> = sqlx::query_scalar(
            "SELECT sequence FROM audit_records WHERE source = $1 AND event_id = $2",
        )
        .bind(event.source())
        .bind(event.event_id().to_string())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(storage)?;
        if let Some(existing) = existing {
            transaction.commit().await.map_err(storage)?;
            return Ok(u64::try_from(existing).unwrap_or_default());
        }
        let head_sequence: i64 = head.try_get("sequence").map_err(storage)?;
        let previous_hash: String = head.try_get("hash").map_err(storage)?;
        let next = u64::try_from(head_sequence).unwrap_or_default() + 1;
        let entry = Entry::from_event(next, event, Utc::now());
        let hash = entry.hash(&previous_hash);
        sqlx::query(concat!(
            "INSERT INTO audit_records (",
            columns!(),
            ") VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)"
        ))
        .bind(sequence(next)?)
        .bind(&entry.event_id)
        .bind(&entry.source)
        .bind(&entry.event_type)
        .bind(i32::try_from(entry.event_version).unwrap_or(i32::MAX))
        .bind(&entry.subject)
        .bind(entry.occurred_at)
        .bind(entry.recorded_at)
        .bind(&entry.actor_type)
        .bind(&entry.actor_id)
        .bind(&entry.correlation_id)
        .bind(&entry.causation_id)
        .bind(&entry.idempotency_key)
        .bind(&entry.traceparent)
        .bind(&entry.data)
        .bind(&previous_hash)
        .bind(&hash)
        .execute(&mut *transaction)
        .await
        .map_err(storage)?;
        sqlx::query("UPDATE audit_head SET sequence = $1, hash = $2")
            .bind(sequence(next)?)
            .bind(&hash)
            .execute(&mut *transaction)
            .await
            .map_err(storage)?;
        if next % CHECKPOINT_EVERY == 0 {
            sqlx::query("INSERT INTO audit_checkpoints (sequence, hash) VALUES ($1, $2)")
                .bind(sequence(next)?)
                .bind(&hash)
                .execute(&mut *transaction)
                .await
                .map_err(storage)?;
        }
        transaction.commit().await.map_err(storage)?;
        Ok(next)
    }

    /// Records matching `filter`, newest first.
    pub async fn list(&self, filter: &Filter) -> Result<Vec<Record>> {
        let rows = sqlx::query(concat!(
            "SELECT ",
            columns!(),
            " FROM audit_records \
             WHERE ($1::bigint IS NULL OR sequence < $1) \
             AND ($2 = '' OR actor_id = $2) \
             AND ($3 = '' OR event_type = $3 \
                  OR (right($3, 1) = '.' AND starts_with(event_type, $3))) \
             AND ($4 = '' OR subject = $4) \
             AND ($5 = '' OR correlation_id = $5) \
             AND ($6::timestamptz IS NULL OR occurred_at >= $6) \
             AND ($7::timestamptz IS NULL OR occurred_at < $7) \
             ORDER BY sequence DESC LIMIT $8"
        ))
        .bind(filter.before.map(sequence).transpose()?)
        .bind(&filter.actor_id)
        .bind(&filter.event_type)
        .bind(&filter.subject)
        .bind(&filter.correlation_id)
        .bind(filter.since)
        .bind(filter.until)
        .bind(i64::from(filter.limit))
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.iter().map(record).collect()
    }

    pub async fn get(&self, number: u64) -> Result<Record> {
        let row = sqlx::query(concat!(
            "SELECT ",
            columns!(),
            " FROM audit_records WHERE sequence = $1"
        ))
        .bind(sequence(number)?)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?
        .ok_or_else(|| PanelError::not_found(format!("there is no audit record {number}")))?;
        record(&row)
    }

    /// Recomputes the hashes of records `from..=to`, the whole chain by
    /// default, and compares them with the stored links, the checkpoints and
    /// the head.
    pub async fn verify(&self, from: Option<u64>, to: Option<u64>) -> Result<Verification> {
        let head = sqlx::query("SELECT sequence, hash FROM audit_head")
            .fetch_one(&self.pool)
            .await
            .map_err(storage)?;
        let head_sequence =
            u64::try_from(head.try_get::<i64, _>("sequence").map_err(storage)?).unwrap_or_default();
        let head_hash: String = head.try_get("hash").map_err(storage)?;
        let first = from.unwrap_or(1).max(1);
        let last = to.unwrap_or(head_sequence).min(head_sequence);
        let mut previous = if first == 1 {
            String::new()
        } else {
            self.get(first - 1).await?.hash
        };
        let checkpoints: Vec<(i64, String)> = sqlx::query_as(
            "SELECT sequence, hash FROM audit_checkpoints WHERE sequence BETWEEN $1 AND $2",
        )
        .bind(sequence(first)?)
        .bind(sequence(last)?)
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        let checkpoints: std::collections::BTreeMap<u64, String> = checkpoints
            .into_iter()
            .map(|(number, hash)| (u64::try_from(number).unwrap_or_default(), hash))
            .collect();
        let mut expected = first;
        let mut checked = 0;
        while expected <= last {
            let rows = sqlx::query(concat!(
                "SELECT ",
                columns!(),
                " FROM audit_records WHERE sequence >= $1 AND sequence <= $2 \
                 ORDER BY sequence LIMIT $3"
            ))
            .bind(sequence(expected)?)
            .bind(sequence(last)?)
            .bind(VERIFY_BATCH)
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?;
            if rows.is_empty() {
                break;
            }
            for row in &rows {
                let stored = record(row)?;
                let computed = stored.entry.hash(&previous);
                let checkpoint_differs = checkpoints
                    .get(&stored.entry.sequence)
                    .is_some_and(|hash| *hash != computed);
                if stored.entry.sequence != expected
                    || stored.previous_hash != previous
                    || stored.hash != computed
                    || checkpoint_differs
                {
                    return Ok(Verification {
                        checked,
                        first_mismatch: Some(expected),
                        head_sequence,
                        head_hash,
                    });
                }
                previous = computed;
                expected += 1;
                checked += 1;
            }
        }
        let missing = expected <= last;
        let head_differs = last == head_sequence && last >= first && previous != head_hash;
        Ok(Verification {
            checked,
            first_mismatch: (missing || head_differs).then_some(expected.min(last)),
            head_sequence,
            head_hash,
        })
    }
}
