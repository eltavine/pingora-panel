use crate::{
    error::ApiError, request_context::command_context, ActivateRequest, ActivatedResponse,
    GatewayStatusResponse, IdempotencyReceiptPendingResponse, IdempotencyReceiptResponse,
    PreparedResponse, ProblemDetails, ReceiptOutcomeResponse, SnapshotEnvelope, ValidationResponse,
};
use axum::{
    extract::{rejection::JsonRejection, DefaultBodyLimit, Json, Path, State},
    http::{header, HeaderMap, HeaderName, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use panel_application::{
    ConfigDocument, ContentHash, GatewayUseCases, IdempotencyKey, IdempotencyLookup,
};
use panel_errors::PanelError;
use std::sync::Arc;
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};
use utoipa::OpenApi;

const DEFAULT_MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Resource policy for the public HTTP adapter.
///
/// Private fields plus validated construction allow future limits to be added
/// without exposing a public struct layout to callers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApiConfig {
    max_body_bytes: usize,
}

impl ApiConfig {
    pub fn new(max_body_bytes: usize) -> Result<Self, PanelError> {
        if max_body_bytes == 0 {
            return Err(PanelError::invalid_argument(
                "API request body limit must be non-zero",
            ));
        }
        Ok(Self { max_body_bytes })
    }

    pub fn max_body_bytes(self) -> usize {
        self.max_body_bytes
    }
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
        }
    }
}

pub struct ApiState<U> {
    use_cases: Arc<U>,
}

impl<U> Clone for ApiState<U> {
    fn clone(&self) -> Self {
        Self {
            use_cases: Arc::clone(&self.use_cases),
        }
    }
}

impl<U> ApiState<U> {
    pub fn new(use_cases: Arc<U>) -> Self {
        Self { use_cases }
    }
}

#[derive(OpenApi)]
#[openapi(
    info(title = "Pingora Panel API", version = "v1"),
    paths(validate, prepare, activate, status, receipt, openapi),
    components(schemas(
        SnapshotEnvelope,
        ActivateRequest,
        ValidationResponse,
        PreparedResponse,
        ActivatedResponse,
        GatewayStatusResponse,
        IdempotencyReceiptPendingResponse,
        IdempotencyReceiptResponse,
        ReceiptOutcomeResponse,
        ProblemDetails
    ))
)]
pub struct ApiDoc;

pub fn router<U>(state: ApiState<U>) -> Router
where
    U: GatewayUseCases + 'static,
{
    router_with_config(state, ApiConfig::default())
}

/// Builds the HTTP adapter with explicit resource policy.
pub fn router_with_config<U>(state: ApiState<U>, config: ApiConfig) -> Router
where
    U: GatewayUseCases + 'static,
{
    let request_id_header = HeaderName::from_static("x-request-id");
    Router::new()
        .route("/api/v1/gateway/validate", post(validate::<U>))
        .route("/api/v1/gateway/prepare", post(prepare::<U>))
        .route("/api/v1/gateway/activate", post(activate::<U>))
        .route("/api/v1/gateway/status", get(status::<U>))
        .route("/api/v1/gateway/receipts/{key}", get(receipt::<U>))
        .route("/api/v1/openapi.json", get(openapi))
        .layer(DefaultBodyLimit::max(config.max_body_bytes()))
        .layer(PropagateRequestIdLayer::new(request_id_header.clone()))
        .layer(TraceLayer::new_for_http())
        .layer(SetRequestIdLayer::new(request_id_header, MakeRequestUuid))
        .with_state(state)
}

#[utoipa::path(
    get,
    path = "/api/v1/gateway/status",
    responses(
        (status = 200, body = GatewayStatusResponse),
        (status = 422, body = ProblemDetails)
    )
)]
async fn status<U>(
    State(state): State<ApiState<U>>,
) -> Result<Json<GatewayStatusResponse>, ApiError>
where
    U: GatewayUseCases,
{
    state
        .use_cases
        .status()
        .await
        .map(GatewayStatusResponse::from)
        .map(Json)
        .map_err(Into::into)
}

#[utoipa::path(
    get,
    path = "/api/v1/gateway/receipts/{key}",
    params(("key" = String, Path, description = "Idempotency key")),
    responses(
        (status = 200, body = IdempotencyReceiptResponse),
        (status = 202, body = IdempotencyReceiptPendingResponse),
        (status = 400, body = ProblemDetails),
        (status = 404, body = ProblemDetails),
        (status = 422, body = ProblemDetails)
    )
)]
async fn receipt<U>(
    State(state): State<ApiState<U>>,
    Path(key): Path<String>,
) -> Result<Response, ApiError>
where
    U: GatewayUseCases,
{
    let key = IdempotencyKey::new(key).map_err(ApiError::new)?;
    let lookup = state
        .use_cases
        .activation_receipt(&key)
        .await
        .map_err(ApiError::new)?;
    project_receipt(lookup)
}

fn project_receipt(lookup: IdempotencyLookup) -> Result<Response, ApiError> {
    match lookup {
        IdempotencyLookup::Missing => Err(ApiError::new(PanelError::not_found(
            "activation receipt not found",
        ))),
        IdempotencyLookup::InProgress => Ok((
            StatusCode::ACCEPTED,
            [(header::RETRY_AFTER, "1")],
            Json(IdempotencyReceiptPendingResponse {
                status: "in_progress".into(),
            }),
        )
            .into_response()),
        IdempotencyLookup::Completed(record) => {
            Ok(Json(IdempotencyReceiptResponse::from(record)).into_response())
        }
        _ => Err(ApiError::new(PanelError::unsupported_capability(
            "unknown activation receipt state",
        ))),
    }
}

#[utoipa::path(
    post,
    path = "/api/v1/gateway/validate",
    request_body = SnapshotEnvelope,
    responses(
        (status = 200, body = ValidationResponse),
        (status = 400, body = ProblemDetails),
        (status = 413, body = ProblemDetails),
        (status = 415, body = ProblemDetails)
    )
)]
async fn validate<U>(
    State(state): State<ApiState<U>>,
    payload: Result<Json<SnapshotEnvelope>, JsonRejection>,
) -> Result<Json<ValidationResponse>, ApiError>
where
    U: GatewayUseCases,
{
    let Json(payload) = payload.map_err(ApiError::from_json)?;
    let document = ConfigDocument::try_from(payload)?;
    state
        .use_cases
        .validate(document)
        .await
        .map(ValidationResponse::from)
        .map(Json)
        .map_err(Into::into)
}

#[utoipa::path(
    post,
    path = "/api/v1/gateway/prepare",
    request_body = SnapshotEnvelope,
    responses(
        (status = 200, body = PreparedResponse),
        (status = 400, body = ProblemDetails),
        (status = 413, body = ProblemDetails),
        (status = 415, body = ProblemDetails),
        (status = 409, body = ProblemDetails),
        (status = 412, body = ProblemDetails),
        (status = 422, body = ProblemDetails),
        (status = 429, body = ProblemDetails)
    )
)]
async fn prepare<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    payload: Result<Json<SnapshotEnvelope>, JsonRejection>,
) -> Result<Json<PreparedResponse>, ApiError>
where
    U: GatewayUseCases,
{
    let context = command_context(&headers)?;
    let request_id = context.request_id().as_str().to_owned();
    let Json(payload) = payload
        .map_err(ApiError::from_json)
        .map_err(|error| error.with_request_id(&request_id))?;
    let document = ConfigDocument::try_from(payload)
        .map_err(|error| ApiError::new(error).with_request_id(&request_id))?;
    state
        .use_cases
        .prepare(context, document)
        .await
        .map(PreparedResponse::from)
        .map(Json)
        .map_err(|error| ApiError::new(error).with_request_id(request_id))
}

#[utoipa::path(
    post,
    path = "/api/v1/gateway/activate",
    request_body = ActivateRequest,
    responses(
        (status = 200, body = ActivatedResponse),
        (status = 400, body = ProblemDetails),
        (status = 413, body = ProblemDetails),
        (status = 415, body = ProblemDetails),
        (status = 409, body = ProblemDetails),
        (status = 412, body = ProblemDetails),
        (status = 422, body = ProblemDetails),
        (status = 429, body = ProblemDetails)
    )
)]
async fn activate<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    payload: Result<Json<ActivateRequest>, JsonRejection>,
) -> Result<Json<ActivatedResponse>, ApiError>
where
    U: GatewayUseCases,
{
    let context = command_context(&headers)?;
    let request_id = context.request_id().as_str().to_owned();
    let Json(payload) = payload
        .map_err(ApiError::from_json)
        .map_err(|error| error.with_request_id(&request_id))?;
    let expected_active_hash = payload
        .expected_active_hash
        .map(ContentHash::from_hex)
        .transpose()
        .map_err(|error| {
            ApiError::new(PanelError::invalid_argument(error.to_string()))
                .with_request_id(&request_id)
        })?;
    state
        .use_cases
        .activate(context, payload.prepare_token, expected_active_hash)
        .await
        .map(ActivatedResponse::from)
        .map(Json)
        .map_err(|error| ApiError::new(error).with_request_id(request_id))
}

#[utoipa::path(
    get,
    path = "/api/v1/openapi.json",
    responses((status = 200, description = "OpenAPI 3.1 document"))
)]
async fn openapi() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}
