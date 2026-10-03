//! Approval policies and the requests applying covered changes opens
//! (ADR 0019).

use crate::{
    configuration::{change, port, read, Precondition},
    error::ApiError,
    request_context::{command_context, MutationHeaders, QueryHeaders},
    ApiState,
};
use axum::{
    body::Bytes,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use panel_application::{ConfigurationChange, GatewayUseCases};
use panel_config_model::{
    ApprovalPolicy, ApprovalPolicyInput, ApprovalRequest, ApprovalRequestList,
};
use panel_errors::PanelError;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct PolicyPath {
    /// The policy's identifier.
    id: String,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct RequestPath {
    /// The request's identifier.
    id: uuid::Uuid,
}

#[derive(Deserialize, IntoParams, Serialize)]
#[into_params(parameter_in = Query)]
pub(crate) struct ApprovalPage {
    /// Only requests opened before this time.
    before: Option<DateTime<Utc>>,
    /// At most this many, 50 by default and 200 at most.
    limit: Option<u32>,
}

/// Why a request is rejected.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct Rejection {
    pub reason: Option<String>,
}

/// Approval policies.
#[utoipa::path(get, path = "/api/v1/approval-policies", params(QueryHeaders),
    responses((status = 200, body = Vec<ApprovalPolicy>)), tag = "approvals")]
pub(crate) async fn list_approval_policies<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    read(
        &state,
        &headers,
        "approval_policies.list",
        "approval-policies".into(),
        None::<&()>,
    )
    .await
}

/// One approval policy.
#[utoipa::path(get, path = "/api/v1/approval-policies/{id}", params(QueryHeaders, PolicyPath),
    responses((status = 200, body = ApprovalPolicy)), tag = "approvals")]
pub(crate) async fn get_approval_policy<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<PolicyPath>,
) -> Result<Response, ApiError> {
    read(
        &state,
        &headers,
        "approval_policies.get",
        format!("approval-policies/{}", path.id),
        None::<&()>,
    )
    .await
}

#[derive(Deserialize)]
struct SavedPolicy {
    policy: serde_json::Value,
    created: bool,
}

/// Creates or replaces an approval policy; every change gives it a new
/// version, which outdates approvals given under the old one.
#[utoipa::path(put, path = "/api/v1/approval-policies/{id}", request_body = ApprovalPolicyInput,
    params(MutationHeaders, PolicyPath),
    responses((status = 200, body = ApprovalPolicy), (status = 201, body = ApprovalPolicy)),
    tag = "approvals")]
pub(crate) async fn put_approval_policy<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<PolicyPath>,
    body: Bytes,
) -> Result<Response, ApiError> {
    let output = port(&state)?
        .change(
            command_context(&headers)?,
            ConfigurationChange {
                operation: "approval_policies.put".into(),
                resource: format!("approval-policies/{}", path.id),
                if_match: None,
                content: body.to_vec(),
            },
        )
        .await?;
    let saved: SavedPolicy = serde_json::from_slice(&output.content)
        .map_err(|_| ApiError::new(PanelError::internal("the saved policy is not readable")))?;
    let status = if saved.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    let mut response = (status, saved.policy.to_string()).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    Ok(response)
}

/// Deletes an approval policy.
#[utoipa::path(delete, path = "/api/v1/approval-policies/{id}",
    params(MutationHeaders, PolicyPath), responses((status = 204)), tag = "approvals")]
pub(crate) async fn delete_approval_policy<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<PolicyPath>,
) -> Result<StatusCode, ApiError> {
    port(&state)?
        .change(
            command_context(&headers)?,
            ConfigurationChange {
                operation: "approval_policies.delete".into(),
                resource: format!("approval-policies/{}", path.id),
                if_match: None,
                content: Vec::new(),
            },
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Approval requests, newest first, with their state as of now.
#[utoipa::path(get, path = "/api/v1/approvals", params(QueryHeaders, ApprovalPage),
    responses((status = 200, body = ApprovalRequestList)), tag = "approvals")]
pub(crate) async fn list_approval_requests<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Query(page): Query<ApprovalPage>,
) -> Result<Response, ApiError> {
    read(
        &state,
        &headers,
        "approvals.list",
        "approvals".into(),
        Some(&page),
    )
    .await
}

/// One approval request.
#[utoipa::path(get, path = "/api/v1/approvals/{id}", params(QueryHeaders, RequestPath),
    responses((status = 200, body = ApprovalRequest)), tag = "approvals")]
pub(crate) async fn get_approval_request<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<RequestPath>,
) -> Result<Response, ApiError> {
    read(
        &state,
        &headers,
        "approvals.get",
        format!("approvals/{}", path.id),
        None::<&()>,
    )
    .await
}

async fn decide<U>(
    state: &ApiState<U>,
    headers: &HeaderMap,
    operation: &str,
    id: uuid::Uuid,
    body: Bytes,
) -> Result<Response, ApiError> {
    change(
        state,
        headers,
        operation,
        format!("approvals/{id}"),
        body,
        Precondition::Optional,
        StatusCode::OK,
    )
    .await
}

/// Approves a request someone else opened, for as long as its policies
/// allow; enough approvals let applying the same content go ahead.
#[utoipa::path(post, path = "/api/v1/approvals/{id}/approve", params(MutationHeaders, RequestPath),
    responses((status = 200, body = ApprovalRequest)), tag = "approvals")]
pub(crate) async fn approve_request<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<RequestPath>,
) -> Result<Response, ApiError> {
    decide(&state, &headers, "approvals.approve", path.id, Bytes::new()).await
}

/// Rejects a request someone else opened.
#[utoipa::path(post, path = "/api/v1/approvals/{id}/reject", request_body = Rejection,
    params(MutationHeaders, RequestPath),
    responses((status = 200, body = ApprovalRequest)), tag = "approvals")]
pub(crate) async fn reject_request<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<RequestPath>,
    body: Bytes,
) -> Result<Response, ApiError> {
    decide(&state, &headers, "approvals.reject", path.id, body).await
}

/// Takes back the caller's approval of a request not yet applied.
#[utoipa::path(post, path = "/api/v1/approvals/{id}/revoke", params(MutationHeaders, RequestPath),
    responses((status = 200, body = ApprovalRequest)), tag = "approvals")]
pub(crate) async fn revoke_approval<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<RequestPath>,
) -> Result<Response, ApiError> {
    decide(&state, &headers, "approvals.revoke", path.id, Bytes::new()).await
}

/// Withdraws the caller's own request.
#[utoipa::path(post, path = "/api/v1/approvals/{id}/withdraw", params(MutationHeaders, RequestPath),
    responses((status = 200, body = ApprovalRequest)), tag = "approvals")]
pub(crate) async fn withdraw_request<U: GatewayUseCases>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<RequestPath>,
) -> Result<Response, ApiError> {
    decide(
        &state,
        &headers,
        "approvals.withdraw",
        path.id,
        Bytes::new(),
    )
    .await
}
