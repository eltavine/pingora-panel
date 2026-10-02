use panel_application::{
    ActivatedDeployment, CommandContext, ConfigDocument, ContentHash, IdempotencyKey,
    PreparedDeployment, RequestId,
};
use panel_domain::RevisionId;
use panel_errors::{PanelError, Result};
use panel_postgres::{storage_error, ServiceDatabase};
use sqlx::PgPool;
use std::time::Duration;

/// A prepared deployment and the document it was prepared from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreparedRecord {
    pub prepare_token: String,
    pub revision_id: RevisionId,
    pub content_hash: ContentHash,
    pub document: ConfigDocument,
}

/// An activation whose idempotency claim has no receipt yet.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingActivation {
    pub idempotency_key: IdempotencyKey,
    pub request_hash: ContentHash,
    pub prepare_token: String,
    pub expected_active_hash: Option<ContentHash>,
    pub actor: String,
    pub correlation_id: RequestId,
}

/// Prepared documents, activation intents and the desired configuration.
#[derive(Clone)]
pub struct PgDeployments {
    pool: PgPool,
}

type PreparedRow = (String, i64, String, String, String, Vec<u8>);

fn hash(value: String) -> Result<ContentHash> {
    ContentHash::from_hex(value)
        .map_err(|error| PanelError::corrupt_state(format!("stored content hash: {error}")))
}

fn prepared(
    (prepare_token, revision_id, content_hash, schema_version, media_type, content): PreparedRow,
) -> Result<PreparedRecord> {
    Ok(PreparedRecord {
        prepare_token,
        revision_id: RevisionId::new(
            u64::try_from(revision_id)
                .map_err(|_| PanelError::corrupt_state("stored revision is negative"))?,
        ),
        content_hash: hash(content_hash)?,
        document: ConfigDocument::new(schema_version, media_type, content)?,
    })
}

fn revision(value: RevisionId) -> Result<i64> {
    i64::try_from(value.get()).map_err(|_| PanelError::invalid_argument("revision is too large"))
}

impl PgDeployments {
    pub fn new(database: &ServiceDatabase) -> Self {
        Self {
            pool: database.pool().clone(),
        }
    }

    pub async fn record_prepared(
        &self,
        prepared: &PreparedDeployment,
        document: &ConfigDocument,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO prepared_deployments \
             (prepare_token, revision_id, content_hash, schema_version, media_type, content) \
             VALUES ($1, $2, $3, $4, $5, $6) ON CONFLICT (prepare_token) DO NOTHING",
        )
        .bind(prepared.prepare_token())
        .bind(revision(prepared.revision_id())?)
        .bind(prepared.content_hash().as_str())
        .bind(document.schema_version())
        .bind(document.media_type())
        .bind(document.body())
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        Ok(())
    }

    /// Records what an activation was asked to do. The first intent for a
    /// key wins, so a conflicting reuse of the key cannot rewrite it.
    pub async fn record_intent(
        &self,
        context: &CommandContext,
        prepare_token: &str,
        expected_active_hash: Option<&ContentHash>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO activation_intents \
             (idempotency_key, prepare_token, expected_active_hash, actor, correlation_id) \
             VALUES ($1, $2, $3, $4, $5) ON CONFLICT (idempotency_key) DO NOTHING",
        )
        .bind(context.idempotency_key().as_str())
        .bind(prepare_token)
        .bind(expected_active_hash.map(ContentHash::as_str))
        .bind(context.actor())
        .bind(context.correlation_id().as_str())
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        Ok(())
    }

    /// Makes an activated deployment the desired configuration unless a
    /// newer revision already is.
    pub async fn record_activated(
        &self,
        prepare_token: &str,
        activated: &ActivatedDeployment,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO desired_configuration (prepare_token, revision_id, content_hash) \
             VALUES ($1, $2, $3) ON CONFLICT (singleton) DO UPDATE \
             SET prepare_token = EXCLUDED.prepare_token, revision_id = EXCLUDED.revision_id, \
                 content_hash = EXCLUDED.content_hash, activated_at = now() \
             WHERE desired_configuration.revision_id < EXCLUDED.revision_id",
        )
        .bind(prepare_token)
        .bind(revision(activated.revision_id())?)
        .bind(activated.content_hash().as_str())
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        Ok(())
    }

    pub async fn desired(&self) -> Result<Option<PreparedRecord>> {
        sqlx::query_as::<_, PreparedRow>(
            "SELECT p.prepare_token, p.revision_id, p.content_hash, p.schema_version, \
                    p.media_type, p.content \
             FROM desired_configuration d \
             JOIN prepared_deployments p ON p.prepare_token = d.prepare_token",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?
        .map(prepared)
        .transpose()
    }

    /// The newest prepared deployment with `content_hash`.
    pub async fn prepared_with_hash(
        &self,
        content_hash: &ContentHash,
    ) -> Result<Option<PreparedRecord>> {
        sqlx::query_as::<_, PreparedRow>(
            "SELECT prepare_token, revision_id, content_hash, schema_version, media_type, content \
             FROM prepared_deployments WHERE content_hash = $1 \
             ORDER BY revision_id DESC LIMIT 1",
        )
        .bind(content_hash.as_str())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?
        .map(prepared)
        .transpose()
    }

    /// Activations that claimed their key but recorded no receipt.
    pub async fn pending(&self) -> Result<Vec<PendingActivation>> {
        let rows: Vec<(String, String, String, Option<String>, String, String)> = sqlx::query_as(
            "SELECT r.idempotency_key, r.request_hash, i.prepare_token, \
                    i.expected_active_hash, i.actor, i.correlation_id \
             FROM activation_receipts r \
             JOIN activation_intents i ON i.idempotency_key = r.idempotency_key \
             WHERE r.receipt IS NULL ORDER BY r.claimed_at",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(storage_error)?;
        rows.into_iter()
            .map(
                |(key, request_hash, prepare_token, expected, actor, correlation_id)| {
                    Ok(PendingActivation {
                        idempotency_key: IdempotencyKey::new(key)?,
                        request_hash: hash(request_hash)?,
                        prepare_token,
                        expected_active_hash: expected.map(hash).transpose()?,
                        actor,
                        correlation_id: RequestId::new(correlation_id)?,
                    })
                },
            )
            .collect()
    }

    /// Removes records older than `retention` that no pending activation or
    /// the desired configuration needs.
    pub async fn purge(&self, retention: Duration) -> Result<u64> {
        let seconds = f64::from(u32::try_from(retention.as_secs()).unwrap_or(u32::MAX));
        let intents = sqlx::query(
            "DELETE FROM activation_intents i WHERE i.recorded_at < now() - make_interval(secs => $1) \
             AND NOT EXISTS (SELECT 1 FROM activation_receipts r \
                             WHERE r.idempotency_key = i.idempotency_key AND r.receipt IS NULL)",
        )
        .bind(seconds)
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        let prepared = sqlx::query(
            "DELETE FROM prepared_deployments p WHERE p.prepared_at < now() - make_interval(secs => $1) \
             AND NOT EXISTS (SELECT 1 FROM desired_configuration d WHERE d.prepare_token = p.prepare_token) \
             AND NOT EXISTS (SELECT 1 FROM activation_intents i WHERE i.prepare_token = p.prepare_token)",
        )
        .bind(seconds)
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        Ok(intents.rows_affected() + prepared.rows_affected())
    }
}
