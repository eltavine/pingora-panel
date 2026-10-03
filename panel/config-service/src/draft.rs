use crate::language;
use chrono::{DateTime, Utc};
use panel_application::{ContentHash, IdempotencyKey, RequestScope};
use panel_config_dsl::Sources;
use panel_config_model::{ConfigModel, MODEL_VERSION};
use panel_errors::{PanelError, Result};
use panel_postgres::{storage_error, EventLog, PgOutbox, ServiceDatabase};
use serde_json::json;
use sqlx::{PgConnection, PgPool};

/// The aggregate of draft events.
pub const DRAFT: (&str, &str) = ("configuration", "draft");

/// The draft and its application state.
#[derive(Clone, Debug)]
pub struct DraftState {
    pub version: u64,
    pub model: ConfigModel,
    /// The draft in the configuration language.
    pub sources: Sources,
    pub updated_at: DateTime<Utc>,
    pub applied_version: Option<u64>,
    pub applied_at: Option<DateTime<Utc>>,
}

/// What one change produced; replayed for a repeated idempotency key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChangeOutput {
    pub content: Vec<u8>,
    pub etag: String,
}

/// A change's new draft. Files left out follow the model: only the blocks of
/// what changed are rewritten.
pub struct DraftChange {
    pub model: ConfigModel,
    pub sources: Option<Sources>,
    pub output: ChangeOutput,
}

/// Identifies one change for idempotency and events.
pub struct ChangeRequest<'a> {
    pub idempotency_key: &'a IdempotencyKey,
    pub operation: &'a str,
    pub resource: &'a str,
    /// Hash of the operation, resource, precondition and body.
    pub request_hash: ContentHash,
    pub scope: &'a RequestScope,
    pub actor: &'a str,
}

/// The draft document in the service schema.
#[derive(Clone)]
pub struct PgDrafts {
    pool: PgPool,
    events: EventLog,
}

type DraftRow = (
    i64,
    String,
    String,
    Option<String>,
    DateTime<Utc>,
    Option<i64>,
    Option<DateTime<Utc>>,
);

fn version(value: i64) -> Result<u64> {
    u64::try_from(value).map_err(|_| PanelError::corrupt_state("stored draft version is negative"))
}

fn stored(version: u64) -> Result<i64> {
    i64::try_from(version).map_err(|_| PanelError::resource_exhausted("draft version overflow"))
}

impl PgDrafts {
    pub fn new(database: &ServiceDatabase, events: EventLog) -> Self {
        Self {
            pool: database.pool().clone(),
            events,
        }
    }

    pub async fn load(&self) -> Result<DraftState> {
        let mut connection = self.pool.acquire().await.map_err(storage_error)?;
        read(&mut connection, false).await
    }

    /// Applies `change` to the current draft while holding its row lock. A
    /// repeated idempotency key returns the recorded output when the request
    /// is the same and is refused when it differs.
    pub async fn change(
        &self,
        request: ChangeRequest<'_>,
        change: impl FnOnce(&DraftState) -> Result<DraftChange>,
    ) -> Result<(DraftState, ChangeOutput)> {
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        let current = read(&mut transaction, true).await?;
        let recorded: Option<(String, Vec<u8>, String)> = sqlx::query_as(
            "SELECT request_hash, content, etag FROM change_receipts WHERE idempotency_key = $1",
        )
        .bind(request.idempotency_key.as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(storage_error)?;
        if let Some((hash, content, etag)) = recorded {
            if hash != request.request_hash.as_str() {
                return Err(PanelError::conflict(
                    "the idempotency key was already used for a different change",
                ));
            }
            return Ok((current, ChangeOutput { content, etag }));
        }
        let DraftChange {
            model,
            sources,
            output,
        } = change(&current)?;
        let sources =
            sources.unwrap_or_else(|| language::follow(&current.sources, &current.model, &model));
        let next = current.version + 1;
        let document = serde_json::to_string(&model)
            .map_err(|error| PanelError::internal(format!("draft cannot be encoded: {error}")))?;
        let files = serde_json::to_string(&sources).map_err(|error| {
            PanelError::internal(format!("draft files cannot be encoded: {error}"))
        })?;
        let (updated_at,): (DateTime<Utc>,) = sqlx::query_as(
            "UPDATE draft_configuration SET version = $1, format = $2, document = $3::jsonb, \
             sources = $4::jsonb, updated_at = now() RETURNING updated_at",
        )
        .bind(stored(next)?)
        .bind(MODEL_VERSION)
        .bind(document)
        .bind(files)
        .fetch_one(&mut *transaction)
        .await
        .map_err(storage_error)?;
        sqlx::query(
            "INSERT INTO change_receipts \
             (idempotency_key, operation, resource, request_hash, content, etag, version) \
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(request.idempotency_key.as_str())
        .bind(request.operation)
        .bind(request.resource)
        .bind(request.request_hash.as_str())
        .bind(&output.content)
        .bind(&output.etag)
        .bind(stored(next)?)
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let event = self.events.event_named(
            "config.draft.changed",
            DRAFT,
            request.scope,
            request.actor,
            &json!({
                "version": next,
                "operation": request.operation,
                "resource": request.resource,
            }),
        )?;
        PgOutbox::append(&mut transaction, &event).await?;
        transaction.commit().await.map_err(storage_error)?;
        Ok((
            DraftState {
                version: next,
                model,
                sources,
                updated_at,
                ..current
            },
            output,
        ))
    }

    /// Records that `version`, as `revision`, now runs on the gateway.
    pub async fn mark_applied(
        &self,
        version: u64,
        revision: u64,
        note: Option<&str>,
        scope: &RequestScope,
        actor: &str,
    ) -> Result<DraftState> {
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        read(&mut transaction, true).await?;
        sqlx::query(
            "UPDATE draft_configuration SET applied_version = $1, applied_at = now() \
             WHERE applied_version IS NULL OR applied_version < $1",
        )
        .bind(stored(version)?)
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let event = self.events.event_named(
            "config.draft.applied",
            DRAFT,
            scope,
            actor,
            &json!({ "version": version, "revision": revision, "note": note }),
        )?;
        PgOutbox::append(&mut transaction, &event).await?;
        let state = read(&mut transaction, false).await?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(state)
    }
}

/// Reads the draft, creating the empty one on first use; `lock` holds the
/// row until the transaction ends.
async fn read(connection: &mut PgConnection, lock: bool) -> Result<DraftState> {
    sqlx::query(
        "INSERT INTO draft_configuration (singleton, version, format, document) \
         VALUES (true, 0, $1, '{}'::jsonb) ON CONFLICT (singleton) DO NOTHING",
    )
    .bind(MODEL_VERSION)
    .execute(&mut *connection)
    .await
    .map_err(storage_error)?;
    let query = if lock {
        "SELECT version, format, document::text, sources::text, updated_at, applied_version, \
         applied_at FROM draft_configuration FOR UPDATE"
    } else {
        "SELECT version, format, document::text, sources::text, updated_at, applied_version, \
         applied_at FROM draft_configuration"
    };
    let (draft_version, format, document, files, updated_at, applied_version, applied_at): DraftRow =
        sqlx::query_as(query)
            .fetch_one(&mut *connection)
            .await
            .map_err(storage_error)?;
    if format != MODEL_VERSION {
        return Err(PanelError::corrupt_state(format!(
            "the draft is stored as {format}, which this service cannot read"
        )));
    }
    let model: ConfigModel = serde_json::from_str(&document)
        .map_err(|error| PanelError::corrupt_state(format!("stored draft is invalid: {error}")))?;
    let sources = match files {
        Some(files) => serde_json::from_str::<std::collections::BTreeMap<String, String>>(&files)
            .map_err(|error| {
                PanelError::corrupt_state(format!("stored draft files are invalid: {error}"))
            })
            .and_then(|files| {
                Sources::new(files).map_err(|path| {
                    PanelError::corrupt_state(format!("stored draft file {path:?} is invalid"))
                })
            })?,
        None => language::printed(&model),
    };
    Ok(DraftState {
        version: version(draft_version)?,
        model,
        sources,
        updated_at,
        applied_version: applied_version.map(version).transpose()?,
        applied_at,
    })
}
