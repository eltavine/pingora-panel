use crate::{
    language,
    store::{ChangeOutput, ChangeRequest, DraftChange, DraftEdit, DraftState, DraftStore, DRAFT},
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use panel_application::RequestScope;
use panel_config_dsl::Sources;
use panel_config_model::{ConfigModel, MODEL_VERSION};
use panel_errors::{PanelError, Result};
use panel_event_contracts::config::v1 as event;
use panel_sqlite::{storage_error, EventLog, ServiceDatabase, SqliteOutbox};
use sqlx::SqliteConnection;

/// The draft document in the module's database.
#[derive(Clone)]
pub struct SqliteDrafts {
    database: ServiceDatabase,
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

impl SqliteDrafts {
    pub fn new(database: &ServiceDatabase, events: EventLog) -> Self {
        Self {
            database: database.clone(),
            events,
        }
    }
}

#[async_trait]
impl DraftStore for SqliteDrafts {
    async fn load(&self) -> Result<DraftState> {
        let mut connection = self
            .database
            .pool()
            .acquire()
            .await
            .map_err(storage_error)?;
        read(&mut connection).await
    }

    async fn change(
        &self,
        request: ChangeRequest<'_>,
        edit: DraftEdit<'_>,
    ) -> Result<(DraftState, ChangeOutput)> {
        let mut transaction = self.database.begin().await?;
        let current = read(&mut transaction).await?;
        let recorded: Option<(String, Vec<u8>, String)> = sqlx::query_as(
            "SELECT request_hash, content, etag FROM change_receipts WHERE idempotency_key = ?1",
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
        } = edit(&current)?;
        let next = current.version + 1;
        let document = serde_json::to_string(&model)
            .map_err(|error| PanelError::internal(format!("draft cannot be encoded: {error}")))?;
        let files = serde_json::to_string(&sources).map_err(|error| {
            PanelError::internal(format!("draft files cannot be encoded: {error}"))
        })?;
        let (updated_at,): (DateTime<Utc>,) = sqlx::query_as(
            "UPDATE draft_configuration SET version = ?1, format = ?2, document = ?3, \
             sources = ?4, updated_at = strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now') RETURNING updated_at",
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
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
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
        let event = self.events.event(
            DRAFT,
            request.scope,
            request.actor,
            &event::DraftChanged {
                version: next,
                operation: request.operation.to_owned(),
                resource: request.resource.to_owned(),
            },
        )?;
        SqliteOutbox::append(&mut transaction, &event).await?;
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

    async fn mark_applied(
        &self,
        version: u64,
        revision: u64,
        note: Option<&str>,
        plan: &str,
        scope: &RequestScope,
        actor: &str,
    ) -> Result<DraftState> {
        let mut transaction = self.database.begin().await?;
        read(&mut transaction).await?;
        sqlx::query(
            "UPDATE draft_configuration SET applied_version = ?1, applied_at = strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now') \
             WHERE applied_version IS NULL OR applied_version < ?1",
        )
        .bind(stored(version)?)
        .execute(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let event = self.events.event(
            DRAFT,
            scope,
            actor,
            &event::DraftApplied {
                version,
                revision,
                note: note.map(str::to_owned),
                plan: plan.to_owned(),
            },
        )?;
        SqliteOutbox::append(&mut transaction, &event).await?;
        let state = read(&mut transaction).await?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(state)
    }
}

/// Reads the draft, creating the empty one on first use.
async fn read(connection: &mut SqliteConnection) -> Result<DraftState> {
    sqlx::query(
        "INSERT INTO draft_configuration (singleton, version, format, document) \
         VALUES (true, 0, ?1, '{}') ON CONFLICT (singleton) DO NOTHING",
    )
    .bind(MODEL_VERSION)
    .execute(&mut *connection)
    .await
    .map_err(storage_error)?;
    let (draft_version, format, document, files, updated_at, applied_version, applied_at): DraftRow =
        sqlx::query_as(
            "SELECT version, format, document, sources, updated_at, applied_version, \
             applied_at FROM draft_configuration",
        )
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
