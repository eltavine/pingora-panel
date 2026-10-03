//! Workload identities and the exchange of workload tokens for sessions
//! (ADR 0020).

use crate::{
    access::client, error::ApiError, identity::gate, request_context::request_scope, ApiState,
};
use axum::{
    extract::{rejection::JsonRejection, ConnectInfo, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use chrono::{DateTime, Utc};
use panel_errors::PanelError;
use panel_identity::{AccountId, Principal, WorkloadIdentity, WorkloadRequest, WorkloadTrust};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, net::SocketAddr, sync::Arc};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

fn workloads<U>(state: &ApiState<U>) -> Result<Arc<WorkloadIdentity>, ApiError> {
    state.workloads.clone().ok_or_else(|| {
        ApiError::new(PanelError::unavailable(
            "workload identities are not available here",
        ))
    })
}

/// A trust in an issuer's tokens for a service account.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct WorkloadIdentityResponse {
    pub id: String,
    /// The service account the workload acts as.
    pub account_id: Uuid,
    pub issuer: String,
    pub audience: String,
    /// Exactly, or a prefix ending in `*`.
    pub subject: String,
    pub claims: BTreeMap<String, String>,
    pub session_minutes: u32,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<WorkloadTrust> for WorkloadIdentityResponse {
    fn from(trust: WorkloadTrust) -> Self {
        Self {
            id: trust.id,
            account_id: trust.account.as_uuid(),
            issuer: trust.issuer,
            audience: trust.audience,
            subject: trust.subject,
            claims: trust.claims,
            session_minutes: trust.session_minutes,
            enabled: trust.enabled,
            created_at: trust.created_at,
            updated_at: trust.updated_at,
        }
    }
}

const fn fifteen() -> u32 {
    15
}

const fn enabled() -> bool {
    true
}

/// A workload identity as an account manager writes it.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkloadIdentityInput {
    /// The service account the workload acts as.
    pub account_id: Uuid,
    /// The issuer's HTTPS URL, exactly as its tokens name it.
    pub issuer: String,
    /// The audience the token must name.
    pub audience: String,
    /// The subject exactly, or a prefix ending in `*`.
    pub subject: String,
    /// Further claims that must equal these values.
    #[serde(default)]
    pub claims: BTreeMap<String, String>,
    /// Five to sixty minutes.
    #[serde(default = "fifteen")]
    pub session_minutes: u32,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct WorkloadPath {
    /// The workload identity's identifier.
    id: String,
}

/// Workload identities.
#[utoipa::path(get, path = "/api/v1/workload-identities",
    responses((status = 200, body = Vec<WorkloadIdentityResponse>)), tag = "identity")]
pub(crate) async fn list_workload_identities<U>(
    State(state): State<ApiState<U>>,
) -> Result<Json<Vec<WorkloadIdentityResponse>>, ApiError> {
    let trusts = workloads(&state)?.list().await?;
    Ok(Json(trusts.into_iter().map(Into::into).collect()))
}

/// One workload identity.
#[utoipa::path(get, path = "/api/v1/workload-identities/{id}", params(WorkloadPath),
    responses((status = 200, body = WorkloadIdentityResponse)), tag = "identity")]
pub(crate) async fn get_workload_identity<U>(
    State(state): State<ApiState<U>>,
    Path(path): Path<WorkloadPath>,
) -> Result<Json<WorkloadIdentityResponse>, ApiError> {
    Ok(Json(workloads(&state)?.get(&path.id).await?.into()))
}

/// Creates or replaces a workload identity for a service account.
#[utoipa::path(put, path = "/api/v1/workload-identities/{id}", params(WorkloadPath),
    request_body = WorkloadIdentityInput,
    responses((status = 200, body = WorkloadIdentityResponse),
        (status = 201, body = WorkloadIdentityResponse)),
    tag = "identity")]
pub(crate) async fn put_workload_identity<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Extension(principal): Extension<Principal>,
    Path(path): Path<WorkloadPath>,
    payload: Result<Json<WorkloadIdentityInput>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(input) = payload.map_err(ApiError::from_json)?;
    let (trust, created) = workloads(&state)?
        .put(
            &path.id,
            WorkloadRequest {
                account: AccountId::from_uuid(input.account_id),
                issuer: input.issuer,
                audience: input.audience,
                subject: input.subject,
                claims: input.claims,
                session_minutes: input.session_minutes,
                enabled: input.enabled,
            },
            &request_scope(&headers)?,
            principal.actor(),
        )
        .await?;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(WorkloadIdentityResponse::from(trust))).into_response())
}

/// Deletes a workload identity; sessions it opened run out on their own.
#[utoipa::path(delete, path = "/api/v1/workload-identities/{id}", params(WorkloadPath),
    responses((status = 204)), tag = "identity")]
pub(crate) async fn delete_workload_identity<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Extension(principal): Extension<Principal>,
    Path(path): Path<WorkloadPath>,
) -> Result<StatusCode, ApiError> {
    workloads(&state)?
        .delete(&path.id, &request_scope(&headers)?, principal.actor())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A workload's token from its issuer.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkloadExchange {
    pub token: String,
}

/// A short bearer session for a service account.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct WorkloadSession {
    /// Sent as `Authorization: Bearer`; shown only now.
    pub secret: String,
    pub expires_at: DateTime<Utc>,
    /// The service account it acts as.
    pub account: String,
}

/// Exchanges a workload's token from a trusted issuer for a short bearer
/// session of the service account a workload identity names.
#[utoipa::path(post, path = "/api/v1/auth/workload", request_body = WorkloadExchange,
    responses((status = 201, body = WorkloadSession)), tag = "identity")]
pub(crate) async fn exchange_workload_token<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    payload: Result<Json<WorkloadExchange>, JsonRejection>,
) -> Result<Response, ApiError> {
    let gate = gate(&state)?;
    let client = client(
        &headers,
        peer.map(|Extension(ConnectInfo(address))| address),
    );
    gate.admit_login(&client)?;
    let Json(exchange) = payload.map_err(ApiError::from_json)?;
    let login = workloads(&state)?
        .exchange(&exchange.token, &client, &request_scope(&headers)?)
        .await?;
    let mut response = (
        StatusCode::CREATED,
        Json(WorkloadSession {
            secret: login.secret.expose().to_owned(),
            expires_at: login.session.expires_at,
            account: login.account.username.to_string(),
        }),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().expect("static"));
    Ok(response)
}
