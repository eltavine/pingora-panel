//! The configuration language and revision history: the draft as files,
//! checking, formatting, plans, dry runs and revisions.

use crate::{
    configuration::{change, insert_draft, port, read, DraftResponse, Precondition},
    contract::DiagnosticDetails,
    error::ApiError,
    request_context::{command_context, MutationHeaders, QueryHeaders},
    ApiState,
};
use axum::{
    body::Bytes,
    extract::{rejection::JsonRejection, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use panel_application::{ApplyOutcome, ApplyRequest as Apply, GatewayUseCases};
use panel_config_dsl::{plan::Changes, schema::DirectiveSpec};
use panel_config_model::{Revision, RevisionDetail, RevisionList};
use panel_errors::PanelError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use utoipa::{IntoParams, ToSchema};

/// Files of the configuration language by path; `main.conf` is the entry.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ConfigFiles {
    pub files: BTreeMap<String, String>,
}

/// The draft in the configuration language.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct ConfigSource {
    pub language_version: u32,
    pub files: BTreeMap<String, String>,
    /// Warnings about the files, such as deprecated directives.
    pub diagnostics: Vec<DiagnosticDetails>,
}

/// Whether files would be accepted, with every problem found.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct CheckResult {
    pub valid: bool,
    pub diagnostics: Vec<DiagnosticDetails>,
}

/// Files formatted canonically; files with syntax errors are unchanged.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct FormattedFiles {
    pub files: BTreeMap<String, String>,
    /// Syntax errors of the files left unchanged.
    pub diagnostics: Vec<DiagnosticDetails>,
}

/// Every directive of the language, for editors.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct LanguageSchema {
    pub language_version: u32,
    pub directives: Vec<DirectiveSpec>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct DryRunRequest {
    /// Refuses if the draft changed since this version.
    #[serde(default)]
    pub expected_version: Option<u64>,
}

/// A dry run that passed every check, the gateway's preparation included.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct DryRunResponse {
    pub draft: DraftResponse,
    /// Warnings found on the way.
    pub diagnostics: Vec<DiagnosticDetails>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct RevisionNote {
    /// Empty or absent removes the note.
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Deserialize, Serialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct RevisionPage {
    /// Lists revisions older than this one.
    before: Option<u64>,
    /// At most this many, 50 by default and 500 at most.
    limit: Option<u32>,
}

#[derive(Deserialize, Serialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct RevisionDiffQuery {
    /// `previous` (the default), `active`, `draft` or a revision number.
    against: Option<String>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct RevisionPath {
    /// Revision number.
    id: u64,
}

/// The draft's files in the configuration language.
#[utoipa::path(get, path = "/api/v1/config/source", params(QueryHeaders),
    responses((status = 200, body = ConfigSource, headers(("ETag" = String)))), tag = "configuration")]
pub(crate) async fn source<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    read(
        &state,
        &headers,
        "config.source",
        "config/source".into(),
        None::<&()>,
    )
    .await
}

/// Replaces the draft with files of the configuration language. Text with
/// errors is refused with their positions.
#[utoipa::path(put, path = "/api/v1/config/source", request_body = ConfigFiles,
    params(MutationHeaders, ("If-Match" = Option<String>, Header, description = "ETag of the draft that was read")),
    responses((status = 200, body = ConfigSource, headers(("ETag" = String)))), tag = "configuration")]
pub(crate) async fn replace_source<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    change(
        &state,
        &headers,
        "config.source.replace",
        "config/source".into(),
        body,
        Precondition::Optional,
        StatusCode::OK,
    )
    .await
}

/// Checks files without saving them.
#[utoipa::path(post, path = "/api/v1/config/check", request_body = ConfigFiles, params(QueryHeaders),
    responses((status = 200, body = CheckResult)), tag = "configuration")]
pub(crate) async fn check<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    payload: Result<Json<ConfigFiles>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(files) = payload.map_err(ApiError::from_json)?;
    read(
        &state,
        &headers,
        "config.check",
        "config".into(),
        Some(&files),
    )
    .await
}

/// Formats files canonically without saving them.
#[utoipa::path(post, path = "/api/v1/config/format", request_body = ConfigFiles, params(QueryHeaders),
    responses((status = 200, body = FormattedFiles)), tag = "configuration")]
pub(crate) async fn format<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    payload: Result<Json<ConfigFiles>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(files) = payload.map_err(ApiError::from_json)?;
    read(
        &state,
        &headers,
        "config.format",
        "config".into(),
        Some(&files),
    )
    .await
}

/// The language's directives, contexts and arguments, for editor completion.
#[utoipa::path(get, path = "/api/v1/config/schema", params(QueryHeaders),
    responses((status = 200, body = LanguageSchema)), tag = "configuration")]
pub(crate) async fn schema<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    read(
        &state,
        &headers,
        "config.schema",
        "config".into(),
        None::<&()>,
    )
    .await
}

/// What applying the draft would change: resources and file differences
/// against the active revision.
#[utoipa::path(get, path = "/api/v1/config/plan", params(QueryHeaders),
    responses((status = 200, body = Changes)), tag = "configuration")]
pub(crate) async fn plan<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    read(
        &state,
        &headers,
        "config.plan",
        "config".into(),
        None::<&()>,
    )
    .await
}

/// Compiles the draft and prepares it on the gateway without activating it.
#[utoipa::path(post, path = "/api/v1/config/dry-run", request_body = DryRunRequest,
    params(MutationHeaders), responses((status = 200, body = DryRunResponse)), tag = "configuration")]
pub(crate) async fn dry_run<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let context = command_context(&headers)?;
    let request: DryRunRequest = if body.is_empty() {
        DryRunRequest::default()
    } else {
        serde_json::from_slice(&body).map_err(|error| {
            ApiError::new(PanelError::invalid_argument(format!(
                "invalid request body: {error}"
            )))
        })?
    };
    match port(&state)?
        .apply(
            context,
            Apply::new(request.expected_version.unwrap_or(0)).dry_run(),
        )
        .await?
    {
        ApplyOutcome::Checked { draft, report, .. } => {
            let mut response = Json(DryRunResponse {
                draft: (&draft).into(),
                diagnostics: report.diagnostics.into_iter().map(Into::into).collect(),
            })
            .into_response();
            insert_draft(response.headers_mut(), &draft);
            Ok(response)
        }
        ApplyOutcome::Rejected { report, .. } => Err(ApiError::new(
            PanelError::validation_failed("the draft would not run")
                .with_diagnostics(report.diagnostics),
        )),
        _ => Err(ApiError::new(PanelError::internal(
            "unexpected dry run outcome",
        ))),
    }
}

/// Configurations applied or attempted, newest first.
#[utoipa::path(get, path = "/api/v1/revisions", params(QueryHeaders, RevisionPage),
    responses((status = 200, body = RevisionList)), tag = "configuration")]
pub(crate) async fn list_revisions<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Query(page): Query<RevisionPage>,
) -> Result<Response, ApiError> {
    read(
        &state,
        &headers,
        "revisions.list",
        "revisions".into(),
        Some(&page),
    )
    .await
}

/// A revision with its files.
#[utoipa::path(get, path = "/api/v1/revisions/{id}", params(QueryHeaders, RevisionPath),
    responses((status = 200, body = RevisionDetail)), tag = "configuration")]
pub(crate) async fn get_revision<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<RevisionPath>,
) -> Result<Response, ApiError> {
    read(
        &state,
        &headers,
        "revisions.get",
        format!("revisions/{}", path.id),
        None::<&()>,
    )
    .await
}

/// What a revision changed relative to another, the draft or the active one.
#[utoipa::path(get, path = "/api/v1/revisions/{id}/diff", params(QueryHeaders, RevisionPath, RevisionDiffQuery),
    responses((status = 200, body = Changes)), tag = "configuration")]
pub(crate) async fn diff_revision<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<RevisionPath>,
    Query(query): Query<RevisionDiffQuery>,
) -> Result<Response, ApiError> {
    read(
        &state,
        &headers,
        "revisions.diff",
        format!("revisions/{}", path.id),
        Some(&query),
    )
    .await
}

/// Copies a revision's files into the draft, to apply them again.
#[utoipa::path(post, path = "/api/v1/revisions/{id}/restore", params(MutationHeaders, RevisionPath),
    responses((status = 200, body = ConfigSource, headers(("ETag" = String)))), tag = "configuration")]
pub(crate) async fn restore_revision<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<RevisionPath>,
) -> Result<Response, ApiError> {
    change(
        &state,
        &headers,
        "revisions.restore",
        format!("revisions/{}", path.id),
        Bytes::new(),
        Precondition::Optional,
        StatusCode::OK,
    )
    .await
}

/// Sets or removes a revision's note.
#[utoipa::path(put, path = "/api/v1/revisions/{id}/note", request_body = RevisionNote,
    params(MutationHeaders, RevisionPath), responses((status = 200, body = Revision)), tag = "configuration")]
pub(crate) async fn note_revision<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<RevisionPath>,
    body: Bytes,
) -> Result<Response, ApiError> {
    change(
        &state,
        &headers,
        "revisions.note",
        format!("revisions/{}", path.id),
        body,
        Precondition::Optional,
        StatusCode::OK,
    )
    .await
}
