//! Roles given to an account with a scope and conditions (ADR 0021).

use crate::{error::ApiError, identity::gate, request_context::request_scope, ApiState};
use axum::{
    extract::{rejection::JsonRejection, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use chrono::{DateTime, Utc};
use panel_identity::{
    AccountId, Grant, GrantConditions, GrantId, GrantRequest, GrantScope, Principal,
};
use panel_schedule::Window;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

/// Where a grant applies.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum GrantScopeBody {
    Everything,
    /// The sites whose `group` is this; limits only configuration
    /// permissions.
    SiteGroup {
        group: String,
    },
    Site {
        site: Uuid,
    },
}

impl From<GrantScope> for GrantScopeBody {
    fn from(scope: GrantScope) -> Self {
        match scope {
            GrantScope::SiteGroup { group } => Self::SiteGroup { group },
            GrantScope::Site { site } => Self::Site { site },
            _ => Self::Everything,
        }
    }
}

impl From<GrantScopeBody> for GrantScope {
    fn from(scope: GrantScopeBody) -> Self {
        match scope {
            GrantScopeBody::Everything => Self::Everything,
            GrantScopeBody::SiteGroup { group } => Self::SiteGroup { group },
            GrantScopeBody::Site { site } => Self::Site { site },
        }
    }
}

/// When a grant counts; every condition set must hold.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GrantConditionsBody {
    #[serde(default)]
    pub not_after: Option<DateTime<Utc>>,
    /// Networks such as `10.0.0.0/8` the client must be in.
    #[serde(default)]
    pub networks: Vec<String>,
    #[serde(default)]
    pub windows: Vec<Window>,
}

impl From<GrantConditions> for GrantConditionsBody {
    fn from(conditions: GrantConditions) -> Self {
        Self {
            not_after: conditions.not_after,
            networks: conditions.networks,
            windows: conditions.windows,
        }
    }
}

impl From<GrantConditionsBody> for GrantConditions {
    fn from(conditions: GrantConditionsBody) -> Self {
        Self {
            not_after: conditions.not_after,
            networks: conditions.networks,
            windows: conditions.windows,
        }
    }
}

/// A role given to an account with a scope and conditions.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct GrantView {
    pub id: Uuid,
    pub role: String,
    pub scope: GrantScopeBody,
    pub conditions: GrantConditionsBody,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
}

impl From<Grant> for GrantView {
    fn from(grant: Grant) -> Self {
        Self {
            id: grant.id.as_uuid(),
            role: grant.role,
            scope: grant.scope.into(),
            conditions: grant.conditions.into(),
            created_at: grant.created_at,
            created_by: grant.created_by,
        }
    }
}

/// A role to give.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewGrant {
    pub role: String,
    /// Everything when absent.
    #[serde(default)]
    pub scope: Option<GrantScopeBody>,
    #[serde(default)]
    pub conditions: GrantConditionsBody,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct AccountGrants {
    /// The account's ID.
    id: Uuid,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct AccountGrant {
    /// The account's ID.
    id: Uuid,
    /// The grant's ID.
    grant: Uuid,
}

/// An account's grants.
#[utoipa::path(get, path = "/api/v1/accounts/{id}/grants", params(AccountGrants),
    responses((status = 200, body = Vec<GrantView>)), tag = "identity")]
pub(crate) async fn list_grants<U>(
    State(state): State<ApiState<U>>,
    Path(path): Path<AccountGrants>,
) -> Result<Json<Vec<GrantView>>, ApiError> {
    let grants = gate(&state)?
        .identity
        .grants(AccountId::from_uuid(path.id))
        .await?;
    Ok(Json(grants.into_iter().map(Into::into).collect()))
}

/// Gives an account a role with a scope and conditions.
#[utoipa::path(post, path = "/api/v1/accounts/{id}/grants", params(AccountGrants),
    request_body = NewGrant, responses((status = 201, body = GrantView)), tag = "identity")]
pub(crate) async fn create_grant<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Extension(principal): Extension<Principal>,
    Path(path): Path<AccountGrants>,
    payload: Result<Json<NewGrant>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(request) = payload.map_err(ApiError::from_json)?;
    let grant = gate(&state)?
        .identity
        .grant(
            AccountId::from_uuid(path.id),
            GrantRequest {
                role: request.role,
                scope: request.scope.map_or(GrantScope::Everything, Into::into),
                conditions: request.conditions.into(),
            },
            &request_scope(&headers)?,
            principal.actor(),
        )
        .await?;
    Ok((StatusCode::CREATED, Json(GrantView::from(grant))).into_response())
}

/// Takes a grant back.
#[utoipa::path(delete, path = "/api/v1/accounts/{id}/grants/{grant}", params(AccountGrant),
    responses((status = 204)), tag = "identity")]
pub(crate) async fn delete_grant<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Extension(principal): Extension<Principal>,
    Path(path): Path<AccountGrant>,
) -> Result<StatusCode, ApiError> {
    gate(&state)?
        .identity
        .revoke_grant(
            AccountId::from_uuid(path.id),
            GrantId::from_uuid(path.grant),
            &request_scope(&headers)?,
            principal.actor(),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
