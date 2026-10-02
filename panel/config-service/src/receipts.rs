use async_trait::async_trait;
use config_proto_codec::{decode_receipt, encode_receipt};
use panel_application::{
    ContentHash, IdempotencyClaim, IdempotencyKey, IdempotencyLookup, IdempotencyRecord,
    IdempotencyRepository,
};
use panel_contracts::config::v1::ActivationReceipt;
use panel_errors::{PanelError, Result};
use panel_postgres::{storage_error, ServiceDatabase};
use prost::Message;
use sqlx::PgPool;

/// Activation receipts in the service schema.
///
/// The first claim of a key inserts its row; completion stores the receipt
/// once, and only a claim that failed before commit is released. Claims and
/// receipts are linearized by the primary key.
pub struct PgActivationReceipts {
    pool: PgPool,
}

impl PgActivationReceipts {
    pub fn new(database: &ServiceDatabase) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    async fn row(&self, key: &IdempotencyKey) -> Result<Option<(String, Option<Vec<u8>>)>> {
        sqlx::query_as(
            "SELECT request_hash, receipt FROM activation_receipts WHERE idempotency_key = $1",
        )
        .bind(key.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)
    }
}

fn decode(bytes: &[u8]) -> Result<IdempotencyRecord> {
    let receipt = ActivationReceipt::decode(bytes).map_err(|error| {
        PanelError::corrupt_state(format!("stored activation receipt is undecodable: {error}"))
    })?;
    decode_receipt(receipt)
}

#[async_trait]
impl IdempotencyRepository for PgActivationReceipts {
    async fn claim(
        &self,
        key: &IdempotencyKey,
        request_hash: &ContentHash,
    ) -> Result<IdempotencyClaim> {
        let inserted = sqlx::query(
            "INSERT INTO activation_receipts (idempotency_key, request_hash) VALUES ($1, $2) \
             ON CONFLICT (idempotency_key) DO NOTHING",
        )
        .bind(key.as_str())
        .bind(request_hash.as_str())
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        if inserted.rows_affected() == 1 {
            return Ok(IdempotencyClaim::Acquired);
        }
        match self.row(key).await? {
            Some((stored, _)) if stored != request_hash.as_str() => Ok(IdempotencyClaim::Conflict),
            Some((_, Some(receipt))) => Ok(IdempotencyClaim::Replay(decode(&receipt)?)),
            // The holder is still running, or released the claim since our
            // insert; either way this attempt must not run concurrently.
            Some((_, None)) | None => Ok(IdempotencyClaim::InProgress),
        }
    }

    async fn complete(&self, key: &IdempotencyKey, record: IdempotencyRecord) -> Result<()> {
        let receipt = encode_receipt(&record)?.encode_to_vec();
        let updated = sqlx::query(
            "UPDATE activation_receipts SET receipt = $3, completed_at = now() \
             WHERE idempotency_key = $1 AND request_hash = $2 AND receipt IS NULL",
        )
        .bind(key.as_str())
        .bind(record.request_hash().as_str())
        .bind(receipt)
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        if updated.rows_affected() == 1 {
            Ok(())
        } else {
            Err(PanelError::precondition_failed(
                "the activation claim to complete does not exist",
            ))
        }
    }

    async fn abort(&self, key: &IdempotencyKey, request_hash: &ContentHash) -> Result<()> {
        sqlx::query(
            "DELETE FROM activation_receipts \
             WHERE idempotency_key = $1 AND request_hash = $2 AND receipt IS NULL",
        )
        .bind(key.as_str())
        .bind(request_hash.as_str())
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        Ok(())
    }

    async fn lookup(&self, key: &IdempotencyKey) -> Result<IdempotencyLookup> {
        Ok(match self.row(key).await? {
            None => IdempotencyLookup::Missing,
            Some((_, None)) => IdempotencyLookup::InProgress,
            Some((_, Some(receipt))) => IdempotencyLookup::Completed(decode(&receipt)?),
        })
    }
}
