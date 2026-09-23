use crate::{
    error::ApiError, request_context::command_context, AbortRequest, AbortResponse,
    ActivateRequest, ActivatedResponse, ApiDoc, ApiState, GatewayStatusResponse,
    IdempotencyReceiptPendingResponse, IdempotencyReceiptResponse, PreparedResponse,
    SnapshotEnvelope, ValidationResponse,
};
use axum::{
    extract::{rejection::JsonRejection, Json, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use panel_application::{
    ConfigDocument, ContentHash, GatewayUseCases, IdempotencyKey, IdempotencyLookup,
};
use panel_errors::PanelError;
use utoipa::OpenApi;

#[utoipa::path(
    get,
    path = "/api/v1/gateway/status",
    responses(
        (status = 200, body = GatewayStatusResponse)
    )
)]
pub(crate) async fn status<U>(
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
        (status = 202, body = IdempotencyReceiptPendingResponse,
            headers(("Retry-After" = String, description = "Suggested delay in seconds before querying again")))
    )
)]
pub(crate) async fn receipt<U>(
    State(state): State<ApiState<U>>,
    key: Result<Path<String>, axum::extract::rejection::PathRejection>,
) -> Result<Response, ApiError>
where
    U: GatewayUseCases,
{
    let Path(key) =
        key.map_err(|error| ApiError::new(PanelError::invalid_argument(error.body_text())))?;
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
        (status = 200, body = ValidationResponse)
    )
)]
pub(crate) async fn validate<U>(
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
    params(crate::request_context::MutationHeaders),
    responses(
        (status = 200, body = PreparedResponse)
    )
)]
pub(crate) async fn prepare<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    payload: Result<Json<SnapshotEnvelope>, JsonRejection>,
) -> Result<Json<PreparedResponse>, ApiError>
where
    U: GatewayUseCases,
{
    let context = command_context(&headers)?;
    let Json(payload) = payload.map_err(ApiError::from_json)?;
    let document = ConfigDocument::try_from(payload)?;
    state
        .use_cases
        .prepare(context, document)
        .await
        .map(PreparedResponse::from)
        .map(Json)
        .map_err(Into::into)
}

#[utoipa::path(
    post,
    path = "/api/v1/gateway/activate",
    request_body = ActivateRequest,
    params(crate::request_context::MutationHeaders),
    responses(
        (status = 200, body = ActivatedResponse)
    )
)]
pub(crate) async fn activate<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    payload: Result<Json<ActivateRequest>, JsonRejection>,
) -> Result<Json<ActivatedResponse>, ApiError>
where
    U: GatewayUseCases,
{
    let context = command_context(&headers)?;
    let Json(payload) = payload.map_err(ApiError::from_json)?;
    let expected_active_hash = payload
        .expected_active_hash
        .map(ContentHash::from_hex)
        .transpose()
        .map_err(|error| ApiError::new(PanelError::invalid_argument(error.to_string())))?;
    state
        .use_cases
        .activate(context, payload.prepare_token, expected_active_hash)
        .await
        .map(ActivatedResponse::from)
        .map(Json)
        .map_err(Into::into)
}

#[utoipa::path(
    post,
    path = "/api/v1/gateway/abort",
    request_body = AbortRequest,
    params(crate::request_context::MutationHeaders),
    responses((status = 200, body = AbortResponse))
)]
pub(crate) async fn abort<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    payload: Result<Json<AbortRequest>, JsonRejection>,
) -> Result<Json<AbortResponse>, ApiError>
where
    U: GatewayUseCases,
{
    let context = command_context(&headers)?;
    let Json(payload) = payload.map_err(ApiError::from_json)?;
    state
        .use_cases
        .abort(context, payload.prepare_token)
        .await
        .map(AbortResponse::from)
        .map(Json)
        .map_err(Into::into)
}

#[utoipa::path(
    get,
    path = "/api/v1/openapi.json",
    responses((status = 200, description = "OpenAPI 3.1 document"))
)]
pub(crate) async fn openapi() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}
