//! The configuration language and revision history: the draft as files,
//! checking, formatting, plans, dry runs and revisions.

use crate::{
    configuration::{change, insert_draft, json, port, read, DraftResponse, Precondition},
    contract::DiagnosticDetails,
    error::ApiError,
    request_context::{command_context, request_scope, MutationHeaders, QueryHeaders},
    ApiState,
};
use axum::{
    body::Bytes,
    extract::{rejection::JsonRejection, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use panel_application::{GatewayUseCases, RequestScope};
use panel_config_api::{
    ApplyOutcome, ApplyRequest as Apply, ConfigurationOutput, ConfigurationPort, LanguageChange,
    LanguageQuery, RevisionChange, RevisionQuery,
};
use panel_config_dsl::{
    plan::Changes, schema::DirectiveSpec, Explanation, SyntaxTree, LANGUAGE_VERSION,
};
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
    /// The draft version these files are.
    pub version: u64,
    /// The draft's entity tag, for `If-Match` when replacing the files.
    pub etag: String,
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

/// Files and the one to read, `main.conf` by default.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct SyntaxRequest {
    pub files: BTreeMap<String, String>,
    #[serde(default)]
    pub file: Option<String>,
}

/// Files and a position in one of them.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ExplainRequest {
    pub files: BTreeMap<String, String>,
    pub file: String,
    /// 1-based.
    pub line: usize,
    /// 1-based, in characters; the first by default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<usize>,
}

/// NGINX configuration files and the one to start from.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct NginxImportRequest {
    /// File contents by path; includes resolve among them.
    pub files: BTreeMap<String, String>,
    /// The main file, such as `nginx.conf`.
    pub entry: String,
}

/// NGINX configuration converted to the language.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct NginxImportResponse {
    pub files: BTreeMap<String, String>,
    /// Directives not carried over, or carried over with another meaning,
    /// at their position in the NGINX files.
    pub report: Vec<DiagnosticDetails>,
    /// Whether the converted files check cleanly.
    pub valid: bool,
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
    read(&state, &headers, LanguageQuery::Source).await
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
        LanguageChange::ReplaceSource {
            files: json::<ConfigFiles>(&headers, &body)?.files,
        },
        Precondition::Optional,
        StatusCode::OK,
    )
    .await
}

/// What a configuration bundle's `format` says.
pub(crate) const BUNDLE_FORMAT: &str = "pingora-panel-configuration";

/// The whole configuration as one file, which another installation imports.
/// Certificates are not in it; TLS profiles name them.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct ConfigBundle {
    /// Always `pingora-panel-configuration`.
    pub format: String,
    /// The language version the files are written in.
    pub language_version: u32,
    pub files: BTreeMap<String, String>,
}

/// The draft as a configuration bundle, sent as an attachment.
#[utoipa::path(get, path = "/api/v1/config/bundle", params(QueryHeaders),
    responses((status = 200, body = ConfigBundle,
        headers(("ETag" = String), ("Content-Disposition" = String)))), tag = "configuration")]
pub(crate) async fn bundle<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let (bundle, output) = draft_bundle(port(&state)?.as_ref(), request_scope(&headers)?).await?;
    let mut response = Json(bundle).into_response();
    let headers = response.headers_mut();
    if let Some(etag) = output
        .etag
        .and_then(|etag| HeaderValue::from_str(&etag).ok())
    {
        headers.insert(header::ETAG, etag);
    }
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename=\"configuration-v{}.json\"",
            output.draft.version
        ))
        .expect("the file name is ASCII"),
    );
    insert_draft(headers, &output.draft);
    Ok(response)
}

/// The draft as a configuration bundle, with what reading it answered.
pub(crate) async fn draft_bundle(
    configuration: &dyn ConfigurationPort,
    scope: RequestScope,
) -> Result<(ConfigBundle, ConfigurationOutput), ApiError> {
    #[derive(Deserialize)]
    struct Source {
        language_version: u32,
        files: BTreeMap<String, String>,
    }
    let output = configuration
        .read(scope, LanguageQuery::Source.into())
        .await?;
    let source: Source = serde_json::from_slice(&output.content).map_err(|_| {
        ApiError::new(PanelError::corrupt_state(
            "the draft's files are unreadable",
        ))
    })?;
    let bundle = ConfigBundle {
        format: BUNDLE_FORMAT.to_owned(),
        language_version: source.language_version,
        files: source.files,
    };
    Ok((bundle, output))
}

/// Refuses a file that is not a configuration bundle this installation
/// reads.
pub(crate) fn importable(bundle: &ConfigBundle) -> Result<(), PanelError> {
    if bundle.format != BUNDLE_FORMAT {
        return Err(PanelError::invalid_argument(format!(
            "this is not a configuration bundle; its format must be {BUNDLE_FORMAT}"
        )));
    }
    if bundle.language_version > LANGUAGE_VERSION {
        return Err(PanelError::validation_failed(format!(
            "the bundle is written in language version {}, newer than this installation's {LANGUAGE_VERSION}",
            bundle.language_version
        )));
    }
    Ok(())
}

/// Replaces the draft with a configuration bundle's files. Bundles written
/// in a newer language version than this installation's are refused.
#[utoipa::path(put, path = "/api/v1/config/bundle", request_body = ConfigBundle,
    params(MutationHeaders, ("If-Match" = Option<String>, Header, description = "ETag of the draft that was read")),
    responses((status = 200, body = ConfigSource, headers(("ETag" = String)))), tag = "configuration")]
pub(crate) async fn import_bundle<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let bundle = json::<ConfigBundle>(&headers, &body)?;
    importable(&bundle).map_err(ApiError::new)?;
    change(
        &state,
        &headers,
        LanguageChange::ReplaceSource {
            files: bundle.files,
        },
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
        LanguageQuery::Check { files: files.files },
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
        LanguageQuery::Format { files: files.files },
    )
    .await
}

/// The syntax tree of a file: its directives as written, without saving.
#[utoipa::path(post, path = "/api/v1/config/ast", request_body = SyntaxRequest, params(QueryHeaders),
    responses((status = 200, body = SyntaxTree)), tag = "configuration")]
pub(crate) async fn ast<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    payload: Result<Json<SyntaxRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(request) = payload.map_err(ApiError::from_json)?;
    read(
        &state,
        &headers,
        LanguageQuery::Syntax {
            files: request.files,
            file: request.file,
        },
    )
    .await
}

/// What applies in the server, route, listener, upstream or TLS profile
/// written at a position, and where each value comes from: the block, a
/// block around it, a listener serving it, or a default, with the rule.
#[utoipa::path(post, path = "/api/v1/config/explain", request_body = ExplainRequest, params(QueryHeaders),
    responses((status = 200, body = Explanation)), tag = "configuration")]
pub(crate) async fn explain<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    payload: Result<Json<ExplainRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(request) = payload.map_err(ApiError::from_json)?;
    read(
        &state,
        &headers,
        LanguageQuery::Explain {
            files: request.files,
            file: request.file,
            line: request.line,
            column: request.column.unwrap_or(1),
        },
    )
    .await
}

/// Converts NGINX configuration to the language without saving it, with a
/// report of everything that did not carry over.
#[utoipa::path(post, path = "/api/v1/config/import/nginx", request_body = NginxImportRequest,
    params(QueryHeaders), responses((status = 200, body = NginxImportResponse)), tag = "configuration")]
pub(crate) async fn import_nginx<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    payload: Result<Json<NginxImportRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(request) = payload.map_err(ApiError::from_json)?;
    read(
        &state,
        &headers,
        LanguageQuery::ImportNginx {
            files: request.files,
            entry: request.entry,
        },
    )
    .await
}

/// The runtime snapshot (IR) the saved draft compiles to, as the gateway
/// would receive it.
#[utoipa::path(get, path = "/api/v1/config/ir", params(QueryHeaders),
    responses((status = 200, description = "The runtime snapshot as JSON", body = Object)),
    tag = "configuration")]
pub(crate) async fn ir<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    read(&state, &headers, LanguageQuery::Ir).await
}

/// The language's directives, contexts and arguments, for editor completion.
#[utoipa::path(get, path = "/api/v1/config/schema", params(QueryHeaders),
    responses((status = 200, body = LanguageSchema)), tag = "configuration")]
pub(crate) async fn schema<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    read(&state, &headers, LanguageQuery::Schema).await
}

/// What applying the draft would change: resources and file differences
/// against the active revision.
#[utoipa::path(get, path = "/api/v1/config/plan", params(QueryHeaders),
    responses((status = 200, body = Changes)), tag = "configuration")]
pub(crate) async fn plan<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    read(&state, &headers, LanguageQuery::Plan).await
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
        RevisionQuery::Revisions {
            before: page.before,
            limit: page.limit,
        },
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
    read(&state, &headers, RevisionQuery::Revision { id: path.id }).await
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
        RevisionQuery::Diff {
            id: path.id,
            against: query
                .against
                .as_deref()
                .map(str::parse)
                .transpose()?
                .unwrap_or_default(),
        },
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
        RevisionChange::Restore { id: path.id },
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
    let note = json::<RevisionNote>(&headers, &body)?.note;
    change(
        &state,
        &headers,
        RevisionChange::Note { id: path.id, note },
        Precondition::Optional,
        StatusCode::OK,
    )
    .await
}
