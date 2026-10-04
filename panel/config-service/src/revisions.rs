//! Every configuration applied or attempted: its files, author, note and
//! outcome. Files never change once recorded.

use crate::store::{NewRevision, RevisionStore};
use async_trait::async_trait;
use panel_config_dsl::{Sources, LANGUAGE_VERSION};
use panel_config_model::{Revision, RevisionOutcome};
use panel_errors::{Diagnostic, PanelError, Result};
use panel_sqlite::{storage_error, ServiceDatabase};
use serde::Serialize;
use sqlx::{sqlite::SqliteRow, Row, SqliteConnection};
use std::collections::BTreeMap;

macro_rules! columns {
    () => {
        "id, draft_version, language_version, content_hash, author, note, created_at, outcome, \
         outcome_at, diagnostics, snapshot_hash, gateway_revision"
    };
}

fn unsigned(value: i64, what: &str) -> Result<u64> {
    u64::try_from(value)
        .map_err(|_| PanelError::corrupt_state(format!("stored {what} is negative")))
}

fn signed(value: u64, what: &str) -> Result<i64> {
    i64::try_from(value)
        .map_err(|_| PanelError::invalid_argument(format!("{what} is out of range")))
}

fn revision(row: &SqliteRow) -> Result<Revision> {
    let column = |error: sqlx::Error| {
        PanelError::corrupt_state(format!("stored revision is invalid: {error}"))
    };
    let diagnostics: Option<String> = row.try_get("diagnostics").map_err(column)?;
    let gateway_revision: Option<i64> = row.try_get("gateway_revision").map_err(column)?;
    let outcome: String = row.try_get("outcome").map_err(column)?;
    Ok(Revision {
        id: unsigned(row.try_get("id").map_err(column)?, "revision id")?,
        draft_version: unsigned(
            row.try_get("draft_version").map_err(column)?,
            "draft version",
        )?,
        language_version: u32::try_from(row.try_get::<i32, _>("language_version").map_err(column)?)
            .map_err(|_| PanelError::corrupt_state("stored language version is negative"))?,
        content_hash: row.try_get("content_hash").map_err(column)?,
        author: row.try_get("author").map_err(column)?,
        note: row.try_get("note").map_err(column)?,
        created_at: row.try_get("created_at").map_err(column)?,
        outcome: RevisionOutcome::parse(&outcome).ok_or_else(|| {
            PanelError::corrupt_state(format!("stored revision outcome {outcome:?} is unknown"))
        })?,
        outcome_at: row.try_get("outcome_at").map_err(column)?,
        diagnostics: diagnostics
            .map(|text| serde_json::from_str(&text))
            .transpose()
            .map_err(|error| {
                PanelError::corrupt_state(format!("stored diagnostics are invalid: {error}"))
            })?
            .unwrap_or_default(),
        snapshot_hash: row.try_get("snapshot_hash").map_err(column)?,
        gateway_revision: gateway_revision
            .map(|value| unsigned(value, "gateway revision"))
            .transpose()?,
    })
}

fn files(text: &str) -> Result<Sources> {
    let files: BTreeMap<String, String> = serde_json::from_str(text).map_err(|error| {
        PanelError::corrupt_state(format!("stored revision files are invalid: {error}"))
    })?;
    Sources::new(files).map_err(|path| {
        PanelError::corrupt_state(format!("stored revision file {path:?} is invalid"))
    })
}

fn encode<T: Serialize + ?Sized>(value: &T) -> Result<String> {
    serde_json::to_string(value)
        .map_err(|error| PanelError::internal(format!("cannot encode: {error}")))
}

#[derive(Clone)]
pub struct SqliteRevisions {
    database: ServiceDatabase,
}

impl SqliteRevisions {
    pub fn new(database: &ServiceDatabase) -> Self {
        Self {
            database: database.clone(),
        }
    }

    async fn insert(
        &self,
        revision: &NewRevision<'_>,
        outcome: &str,
        diagnostics: Option<&[Diagnostic]>,
    ) -> Result<u64> {
        let (id,): (i64,) = sqlx::query_as(
            "INSERT INTO configuration_revisions \
             (draft_version, language_version, sources, content_hash, author, note, outcome, diagnostics, snapshot_hash) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) RETURNING id",
        )
        .bind(signed(revision.draft_version, "draft version")?)
        .bind(i32::try_from(LANGUAGE_VERSION).unwrap_or(i32::MAX))
        .bind(encode(revision.sources)?)
        .bind(revision.content_hash)
        .bind(revision.author)
        .bind(revision.note)
        .bind(outcome)
        .bind(diagnostics.map(encode).transpose()?)
        .bind(revision.snapshot_hash)
        .fetch_one(self.database.pool())
        .await
        .map_err(storage_error)?;
        unsigned(id, "revision id")
    }
}

#[async_trait]
impl RevisionStore for SqliteRevisions {
    async fn begin(&self, revision: &NewRevision<'_>) -> Result<u64> {
        self.insert(revision, "applying", None).await
    }

    async fn reject(&self, revision: &NewRevision<'_>, diagnostics: &[Diagnostic]) -> Result<u64> {
        self.insert(revision, "rejected", Some(diagnostics)).await
    }

    async fn activate(&self, id: u64, snapshot_hash: &str, gateway_revision: u64) -> Result<()> {
        let mut transaction = self.database.begin().await?;
        activate(&mut transaction, id, snapshot_hash, Some(gateway_revision)).await?;
        transaction.commit().await.map_err(storage_error)
    }

    async fn fail(&self, id: u64, diagnostics: &[Diagnostic]) -> Result<()> {
        sqlx::query(
            "UPDATE configuration_revisions SET outcome = 'failed', outcome_at = strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now'), \
             diagnostics = ?2 WHERE id = ?1 AND outcome = 'applying'",
        )
        .bind(signed(id, "revision id")?)
        .bind(encode(&diagnostics)?)
        .execute(self.database.pool())
        .await
        .map_err(storage_error)?;
        Ok(())
    }

    async fn settle(&self, active_hash: Option<&str>) -> Result<()> {
        let mut transaction = self.database.begin().await?;
        let open: Vec<(i64, Option<String>)> = sqlx::query_as(
            "SELECT id, snapshot_hash FROM configuration_revisions WHERE outcome = 'applying' \
             ORDER BY id",
        )
        .fetch_all(&mut *transaction)
        .await
        .map_err(storage_error)?;
        for (id, snapshot_hash) in open {
            let id = unsigned(id, "revision id")?;
            match (snapshot_hash.as_deref(), active_hash) {
                (Some(hash), Some(active)) if hash == active => {
                    activate(&mut transaction, id, hash, None).await?;
                }
                _ => {
                    let interrupted = [Diagnostic::error(
                        "APPLY_INTERRUPTED",
                        "the attempt ended before the gateway confirmed it",
                    )];
                    sqlx::query(
                        "UPDATE configuration_revisions SET outcome = 'failed', outcome_at = strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now'), \
                         diagnostics = ?2 WHERE id = ?1",
                    )
                    .bind(signed(id, "revision id")?)
                    .bind(encode(&interrupted)?)
                    .execute(&mut *transaction)
                    .await
                    .map_err(storage_error)?;
                }
            }
        }
        transaction.commit().await.map_err(storage_error)
    }

    async fn get(&self, id: u64) -> Result<(Revision, Sources)> {
        let row = sqlx::query(concat!(
            "SELECT ",
            columns!(),
            ", sources FROM configuration_revisions WHERE id = ?1"
        ))
        .bind(signed(id, "revision id")?)
        .fetch_optional(self.database.pool())
        .await
        .map_err(storage_error)?
        .ok_or_else(|| PanelError::not_found(format!("no revision {id}")))?;
        let sources: String = row.try_get("sources").map_err(|error| {
            PanelError::corrupt_state(format!("stored revision is invalid: {error}"))
        })?;
        Ok((revision(&row)?, files(&sources)?))
    }

    async fn active(&self) -> Result<Option<(Revision, Sources)>> {
        let id: Option<(i64,)> =
            sqlx::query_as("SELECT id FROM configuration_revisions WHERE outcome = 'active'")
                .fetch_optional(self.database.pool())
                .await
                .map_err(storage_error)?;
        match id {
            Some((id,)) => Ok(Some(self.get(unsigned(id, "revision id")?).await?)),
            None => Ok(None),
        }
    }

    async fn list(&self, before: Option<u64>, limit: u32) -> Result<Vec<Revision>> {
        let rows = sqlx::query(concat!(
            "SELECT ",
            columns!(),
            " FROM configuration_revisions WHERE (?1 IS NULL OR id < ?1) \
             ORDER BY id DESC LIMIT ?2"
        ))
        .bind(before.map(|before| signed(before, "cursor")).transpose()?)
        .bind(i64::from(limit))
        .fetch_all(self.database.pool())
        .await
        .map_err(storage_error)?;
        rows.iter().map(revision).collect()
    }

    async fn set_note(&self, id: u64, note: Option<&str>) -> Result<Revision> {
        let updated = sqlx::query("UPDATE configuration_revisions SET note = ?2 WHERE id = ?1")
            .bind(signed(id, "revision id")?)
            .bind(note)
            .execute(self.database.pool())
            .await
            .map_err(storage_error)?;
        if updated.rows_affected() == 0 {
            return Err(PanelError::not_found(format!("no revision {id}")));
        }
        Ok(self.get(id).await?.0)
    }
}

async fn activate(
    connection: &mut SqliteConnection,
    id: u64,
    snapshot_hash: &str,
    gateway_revision: Option<u64>,
) -> Result<()> {
    sqlx::query(
        "UPDATE configuration_revisions SET outcome = 'superseded', outcome_at = strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now') \
         WHERE outcome = 'active' AND id <> ?1",
    )
    .bind(signed(id, "revision id")?)
    .execute(&mut *connection)
    .await
    .map_err(storage_error)?;
    sqlx::query(
        "UPDATE configuration_revisions SET outcome = 'active', outcome_at = strftime('%Y-%m-%dT%H:%M:%f+00:00', 'now'), \
         snapshot_hash = ?2, gateway_revision = COALESCE(?3, gateway_revision) WHERE id = ?1",
    )
    .bind(signed(id, "revision id")?)
    .bind(snapshot_hash)
    .bind(
        gateway_revision
            .map(|value| signed(value, "gateway revision"))
            .transpose()?,
    )
    .execute(&mut *connection)
    .await
    .map_err(storage_error)?;
    Ok(())
}
