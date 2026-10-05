//! Backups (ADR 0035): taking, listing, downloading, restoring and removing
//! archives of the installation. A backup holding the configuration also
//! carries the draft and the active revision as configuration bundles, and
//! the configuration is restored from the latter as a change of the draft.

use crate::{
    error::ApiError,
    language::{draft_bundle, importable, ConfigBundle, BUNDLE_FORMAT},
    request_context::{command_context, request_scope, MutationHeaders, QueryHeaders},
    ApiState,
};
use axum::{
    body::{Body, Bytes},
    extract::{Path, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use base64::Engine;
use chrono::{DateTime, SecondsFormat, Utc};
use futures_util::StreamExt;
use panel_application::{
    Backup, BackupChange, BackupContent, BackupRequest, BackupState, Operation, RequestScope,
    ACTIVE_BUNDLE, DRAFT_BUNDLE,
};
use panel_config_api::{ConfigurationChange, ConfigurationPort, LanguageChange, RevisionQuery};
use panel_config_model::{RevisionDetail, RevisionList, RevisionOutcome};
use panel_errors::PanelError;
use panel_identity::{Access as HeldAccess, Permission};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc, time::SystemTime};
use utoipa::ToSchema;

/// RFC 9530: the digest of the archive the response carries.
static REPR_DIGEST: HeaderName = HeaderName::from_static("repr-digest");
/// The revisions searched for the active one.
const REVISIONS_SEARCHED: u32 = 100;

/// What a backup holds.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BackupContentName {
    /// The configuration's database, with the draft and the active
    /// revision as configuration bundles.
    Configuration,
    /// Certificates with their keys sealed, ACME accounts and DNS providers.
    Certificates,
    /// Every module's database.
    Databases,
    /// The sites' directory, or one directory below it.
    Sites,
}

impl From<BackupContentName> for BackupContent {
    fn from(name: BackupContentName) -> Self {
        match name {
            BackupContentName::Configuration => Self::Configuration,
            BackupContentName::Certificates => Self::Certificates,
            BackupContentName::Databases => Self::Databases,
            BackupContentName::Sites => Self::Sites,
        }
    }
}

fn content_name(content: BackupContent) -> Option<BackupContentName> {
    match content {
        BackupContent::Configuration => Some(BackupContentName::Configuration),
        BackupContent::Certificates => Some(BackupContentName::Certificates),
        BackupContent::Databases => Some(BackupContentName::Databases),
        BackupContent::Sites => Some(BackupContentName::Sites),
        _ => None,
    }
}

/// How far taking a backup got.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BackupStateName {
    Pending,
    Running,
    Completed,
    Failed,
}

/// Why taking a backup failed.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct BackupFailure {
    pub code: String,
    pub message: String,
}

/// A backup and how far taking it got.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct BackupDetails {
    pub id: String,
    pub contents: Vec<BackupContentName>,
    /// The directory below the sites' directory it holds; absent for all of
    /// it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site_path: Option<String>,
    pub state: BackupStateName,
    pub requested_by: String,
    pub requested_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<String>,
    /// The archive's size, once taken.
    pub size_bytes: u64,
    /// SHA-256 of the archive in lowercase hexadecimal; empty until taken.
    pub sha256: String,
    /// The files in the archive.
    pub files: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<BackupFailure>,
    /// The version of the product that took it.
    pub product_version: String,
}

fn timestamp(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Secs, true)
}

impl From<Backup> for BackupDetails {
    fn from(backup: Backup) -> Self {
        Self {
            contents: backup
                .contents
                .into_iter()
                .filter_map(content_name)
                .collect(),
            site_path: Some(backup.site_path).filter(|path| !path.is_empty()),
            state: match backup.state {
                BackupState::Pending => BackupStateName::Pending,
                BackupState::Running => BackupStateName::Running,
                BackupState::Completed => BackupStateName::Completed,
                _ => BackupStateName::Failed,
            },
            requested_at: timestamp(backup.requested_at),
            finished_at: backup.finished_at.map(timestamp),
            failure: backup.failure.map(|failure| BackupFailure {
                code: failure.code.as_str().to_owned(),
                message: failure.message,
            }),
            id: backup.id,
            requested_by: backup.requested_by,
            size_bytes: backup.size_bytes,
            sha256: backup.sha256,
            files: backup.files,
            product_version: backup.product_version,
        }
    }
}

/// Backups, newest first.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct BackupList {
    pub backups: Vec<BackupDetails>,
}

/// What to back up.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewBackup {
    pub contents: Vec<BackupContentName>,
    /// With the sites, the directory below the sites' directory to hold,
    /// such as `shop`; all of it when absent.
    #[serde(default)]
    pub site_path: Option<String>,
}

/// What to restore from a backup.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(tag = "target", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum BackupRestore {
    /// Replaces a directory below the sites' directory with the backup's
    /// copy.
    Sites { site_path: String },
    /// Saves the backup's active revision as the draft, to be reviewed and
    /// applied as any other; it needs `config.write` as well.
    Configuration,
}

/// What restoring from a backup did.
#[derive(Clone, Debug, Serialize, ToSchema)]
#[serde(tag = "target", rename_all = "snake_case")]
#[non_exhaustive]
pub enum BackupRestored {
    Sites {
        site_path: String,
        files: u64,
        bytes: u64,
    },
    Configuration {
        /// The draft version the configuration was saved as.
        draft_version: u64,
    },
}

fn site_path(value: &str) -> Result<String, ApiError> {
    let path = value.trim_matches('/');
    if path.is_empty() {
        return Err(ApiError::new(PanelError::invalid_argument(
            "name a directory below the sites' directory",
        )));
    }
    Ok(path.to_owned())
}

fn location(id: &str) -> (HeaderName, String) {
    (header::LOCATION, format!("/api/v1/backups/{id}"))
}

/// Every backup, newest first.
#[utoipa::path(get, path = "/api/v1/backups", params(QueryHeaders),
    responses((status = 200, body = BackupList)), tag = "backups")]
pub(crate) async fn list_backups<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Json<BackupList>, ApiError> {
    let backups = state.backups.list(request_scope(&headers)?).await?;
    Ok(Json(BackupList {
        backups: backups.into_iter().map(Into::into).collect(),
    }))
}

/// A backup and how far taking it got.
#[utoipa::path(get, path = "/api/v1/backups/{id}", params(QueryHeaders, ("id" = String, Path)),
    responses((status = 200, body = BackupDetails)), tag = "backups")]
pub(crate) async fn get_backup<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<BackupDetails>, ApiError> {
    let backup = state.backups.get(request_scope(&headers)?, &id).await?;
    Ok(Json(backup.into()))
}

/// The draft and the active revision as configuration bundles, by where
/// a backup keeps them.
async fn configuration_bundles(
    configuration: &dyn ConfigurationPort,
    scope: RequestScope,
) -> Result<BTreeMap<String, Vec<u8>>, ApiError> {
    let encoded = |bundle: &ConfigBundle| {
        serde_json::to_vec_pretty(bundle).expect("configuration bundles serialize")
    };
    let mut bundles = BTreeMap::new();
    let (draft, _) = draft_bundle(configuration, scope.clone()).await?;
    bundles.insert(DRAFT_BUNDLE.to_owned(), encoded(&draft));
    let unreadable = || ApiError::new(PanelError::corrupt_state("the revisions are unreadable"));
    let revisions: RevisionList = serde_json::from_slice(
        &configuration
            .read(
                scope.clone(),
                RevisionQuery::Revisions {
                    before: None,
                    limit: Some(REVISIONS_SEARCHED),
                }
                .into(),
            )
            .await?
            .content,
    )
    .map_err(|_| unreadable())?;
    if let Some(active) = revisions
        .items
        .iter()
        .find(|revision| revision.outcome == RevisionOutcome::Active)
    {
        let detail: RevisionDetail = serde_json::from_slice(
            &configuration
                .read(scope, RevisionQuery::Revision { id: active.id }.into())
                .await?
                .content,
        )
        .map_err(|_| unreadable())?;
        let active = ConfigBundle {
            format: BUNDLE_FORMAT.to_owned(),
            language_version: detail.revision.language_version,
            files: detail.files,
        };
        bundles.insert(ACTIVE_BUNDLE.to_owned(), encoded(&active));
    }
    Ok(bundles)
}

/// Takes a backup in the background. It is listed at once, pending; poll
/// it at `Location` until it is completed or failed. A backup holding the
/// configuration carries the draft and the active revision as bundles.
#[utoipa::path(post, path = "/api/v1/backups", params(MutationHeaders), request_body = NewBackup,
    responses((status = 202, body = BackupDetails, headers(("Location" = String)))), tag = "backups")]
pub(crate) async fn create_backup<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, [(HeaderName, String); 1], Json<BackupDetails>), ApiError> {
    let wanted = crate::configuration::json::<NewBackup>(&headers, &body)?;
    let context = command_context(&headers)?;
    let contents: Vec<BackupContent> = wanted.contents.into_iter().map(Into::into).collect();
    let attachments = match &state.configuration {
        Some(configuration) if contents.iter().any(|content| content.holds_configuration()) => {
            configuration_bundles(configuration.as_ref(), context.scope()).await?
        }
        _ => BTreeMap::new(),
    };
    let request = BackupRequest {
        contents,
        site_path: wanted
            .site_path
            .as_deref()
            .map(site_path)
            .transpose()?
            .unwrap_or_default(),
        attachments,
    };
    let backup = state.backups.create(context, request).await?;
    Ok((
        StatusCode::ACCEPTED,
        [location(&backup.id)],
        Json(backup.into()),
    ))
}

/// Removes a backup and its archive; one being taken is refused.
#[utoipa::path(delete, path = "/api/v1/backups/{id}", params(MutationHeaders, ("id" = String, Path)),
    responses((status = 204)), tag = "backups")]
pub(crate) async fn delete_backup<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state
        .backups
        .delete(command_context(&headers)?, &id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A taken backup's archive: a Zstandard-compressed tar archive whose first
/// member, `manifest.json`, lists every file with its size and SHA-256.
/// `Repr-Digest` carries the archive's own SHA-256.
#[utoipa::path(get, path = "/api/v1/backups/{id}/archive", params(QueryHeaders, ("id" = String, Path)),
    responses((status = 200, content_type = "application/zstd", body = crate::site_files::FileBytes,
        headers(("ETag" = String), ("Repr-Digest" = String), ("Content-Disposition" = String)))),
    tag = "backups")]
pub(crate) async fn download_backup<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, ApiError> {
    let download = state
        .backups
        .download(request_scope(&headers)?, &id)
        .await?;
    let backup = download.backup;
    let digest = hex::decode(&backup.sha256).map_err(|_| {
        ApiError::new(PanelError::corrupt_state(
            "the backup's digest is unreadable",
        ))
    })?;
    let body = Body::from_stream(
        download
            .chunks
            .map(|chunk| chunk.map(Bytes::from).map_err(std::io::Error::other)),
    );
    let mut response = (StatusCode::OK, body).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/zstd"),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(backup.size_bytes));
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename=\"pingora-panel-backup-{}.tar.zst\"",
            backup.id
        ))
        .map_err(|_| ApiError::new(PanelError::corrupt_state("the backup's ID is unreadable")))?,
    );
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(&format!("\"{}\"", backup.sha256))
            .expect("hexadecimal digests are visible ASCII"),
    );
    headers.insert(
        REPR_DIGEST.clone(),
        HeaderValue::from_str(&format!(
            "sha-256=:{}:",
            base64::engine::general_purpose::STANDARD.encode(digest)
        ))
        .expect("base64 is visible ASCII"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

/// Restores a directory of the sites, or the configuration, from a backup.
/// The configuration is saved as the draft, refused if the draft changed
/// since the `If-Match` sent, and reviewed and applied as any other.
#[utoipa::path(post, path = "/api/v1/backups/{id}/restores",
    params(MutationHeaders, ("id" = String, Path),
        ("If-Match" = Option<String>, Header, description = "With the configuration, the ETag of the draft that was read")),
    request_body = BackupRestore, responses((status = 200, body = BackupRestored)), tag = "backups")]
pub(crate) async fn restore_backup<U>(
    State(state): State<ApiState<U>>,
    held: Option<Extension<HeldAccess>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Bytes,
) -> Result<Json<BackupRestored>, ApiError> {
    let wanted = crate::configuration::json::<BackupRestore>(&headers, &body)?;
    let context = command_context(&headers)?;
    match wanted {
        BackupRestore::Sites { site_path: path } => {
            let path = site_path(&path)?;
            let restored = state.backups.restore_sites(context, &id, &path).await?;
            Ok(Json(BackupRestored::Sites {
                site_path: path,
                files: restored.files,
                bytes: restored.bytes,
            }))
        }
        BackupRestore::Configuration => {
            if held
                .as_ref()
                .is_some_and(|Extension(held)| !held.holds(Permission::ConfigWrite))
            {
                return Err(ApiError::new(PanelError::permission_denied(
                    "restoring the configuration needs the config.write permission",
                )));
            }
            let configuration = state.configuration.clone().ok_or_else(|| {
                ApiError::new(PanelError::unavailable(
                    "configuration management is not available here",
                ))
            })?;
            let restored = restore_configuration(
                &state.backups,
                configuration.as_ref(),
                &context,
                &id,
                if_match(&headers)?,
            )
            .await;
            if let Some(operations) = &state.operations {
                let change = BackupChange::ConfigurationRestored(restored.as_ref().copied());
                operations
                    .record(&context, Operation::Backup { id: &id, change })
                    .await;
            }
            Ok(Json(BackupRestored::Configuration {
                draft_version: restored.map_err(ApiError::new)?,
            }))
        }
    }
}

fn if_match(headers: &HeaderMap) -> Result<Option<String>, ApiError> {
    headers
        .get(header::IF_MATCH)
        .map(|value| {
            value.to_str().map(str::to_owned).map_err(|_| {
                ApiError::new(PanelError::invalid_argument(
                    "If-Match must be visible ASCII",
                ))
            })
        })
        .transpose()
}

/// Saves a backup's active revision as the draft, answering the draft's
/// version.
async fn restore_configuration(
    backups: &Arc<dyn panel_application::BackupsPort>,
    configuration: &dyn ConfigurationPort,
    context: &panel_application::CommandContext,
    id: &str,
    if_match: Option<String>,
) -> Result<u64, PanelError> {
    let content = backups
        .member(context.scope(), id, ACTIVE_BUNDLE)
        .await
        .map_err(|error| match error.code.as_str() {
            "NOT_FOUND" => {
                PanelError::not_found("the backup holds no active configuration to restore")
            }
            _ => error,
        })?;
    let bundle: ConfigBundle = serde_json::from_slice(&content)
        .map_err(|_| PanelError::validation_failed("the backup's configuration is not a bundle"))?;
    importable(&bundle)?;
    let saved = configuration
        .change(
            context.clone(),
            ConfigurationChange {
                command: LanguageChange::ReplaceSource {
                    files: bundle.files,
                }
                .into(),
                if_match,
            },
        )
        .await?;
    Ok(saved.draft.version)
}
