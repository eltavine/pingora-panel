//! Identity providers: managing them, and signing in through them with the
//! authorization code flow (ADR 0018).

use crate::{
    access::{client, cookie, session_cookie},
    error::ApiError,
    request_context::request_scope,
    ApiState,
};
use axum::{
    extract::{rejection::JsonRejection, ConnectInfo, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use chrono::{DateTime, SecondsFormat, Utc};
use panel_errors::PanelError;
use panel_identity::{
    ClaimNames, GroupRole, PasswordSignIn, Principal, ProviderDirectory, ProviderRequest,
    ProviderSignIns, ProviderView, SecretChange, Transport,
};
use serde::{Deserialize, Deserializer, Serialize};
use std::{net::SocketAddr, sync::Arc};
use utoipa::{IntoParams, ToSchema};

/// The cookie that binds a sign-in to the browser that started it.
const SIGN_IN_COOKIE: &str = "__Host-ppanel_sign_in";
const SIGN_IN_COOKIE_SECONDS: u64 = 600;

/// Identity providers and sign-ins through them.
pub(crate) struct ProviderAccess {
    pub(crate) directory: ProviderDirectory,
    /// Absent until the panel knows its public origin.
    pub(crate) sign_ins: Option<ProviderSignIns>,
}

fn access<U>(state: &ApiState<U>) -> Result<Arc<ProviderAccess>, ApiError> {
    state.providers.clone().ok_or_else(|| {
        ApiError::new(PanelError::unavailable(
            "identity providers are not available here",
        ))
    })
}

fn timestamp(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// The claims that give a person's attributes.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ClaimNamesBody {
    pub username: String,
    pub display_name: String,
    pub email: String,
    pub groups: String,
}

impl From<ClaimNames> for ClaimNamesBody {
    fn from(claims: ClaimNames) -> Self {
        Self {
            username: claims.username,
            display_name: claims.display_name,
            email: claims.email,
            groups: claims.groups,
        }
    }
}

/// A role that members of a provider's group receive.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct GroupRoleBody {
    pub group: String,
    pub role: String,
}

/// An identity provider; its client secret is never returned.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct IdentityProviderResponse {
    pub id: String,
    pub display_name: String,
    pub issuer: String,
    pub client_id: String,
    /// Whether the client is confidential and has a secret.
    pub has_client_secret: bool,
    pub scopes: Vec<String>,
    pub claims: ClaimNamesBody,
    pub group_roles: Vec<GroupRoleBody>,
    /// Whether people unknown to the panel get an account on first sign-in.
    pub create_accounts: bool,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

impl From<ProviderView> for IdentityProviderResponse {
    fn from(view: ProviderView) -> Self {
        Self {
            id: view.id,
            display_name: view.display_name,
            issuer: view.issuer,
            client_id: view.client_id,
            has_client_secret: view.has_client_secret,
            scopes: view.scopes,
            claims: view.claims.into(),
            group_roles: view
                .group_roles
                .into_iter()
                .map(|mapping| GroupRoleBody {
                    group: mapping.group,
                    role: mapping.role,
                })
                .collect(),
            create_accounts: view.create_accounts,
            enabled: view.enabled,
            created_at: timestamp(view.created_at),
            updated_at: timestamp(view.updated_at),
        }
    }
}

fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(deserializer).map(Some)
}

/// An identity provider as an Administrator writes it.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct IdentityProviderInput {
    pub display_name: String,
    /// The provider's issuer URL, exactly as its tokens name it.
    pub issuer: String,
    pub client_id: String,
    /// A new secret; `null` makes the client public and leaving it out keeps
    /// the current one.
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>)]
    pub client_secret: Option<Option<String>>,
    /// Requested besides `openid`; `profile email` when absent.
    #[serde(default)]
    pub scopes: Option<Vec<String>>,
    #[serde(default)]
    pub claims: Option<ClaimNamesBody>,
    #[serde(default)]
    pub group_roles: Vec<GroupRoleBody>,
    #[serde(default)]
    pub create_accounts: bool,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

const fn enabled() -> bool {
    true
}

impl From<IdentityProviderInput> for ProviderRequest {
    fn from(input: IdentityProviderInput) -> Self {
        Self {
            display_name: input.display_name,
            issuer: input.issuer,
            client_id: input.client_id,
            client_secret: match input.client_secret {
                None => SecretChange::Keep,
                Some(None) => SecretChange::Clear,
                Some(Some(secret)) => SecretChange::Set(secret),
            },
            scopes: input
                .scopes
                .unwrap_or_else(|| vec!["profile".into(), "email".into()]),
            claims: input
                .claims
                .map_or_else(ClaimNames::default, |claims| ClaimNames {
                    username: claims.username,
                    display_name: claims.display_name,
                    email: claims.email,
                    groups: claims.groups,
                }),
            group_roles: input
                .group_roles
                .into_iter()
                .map(|mapping| GroupRole {
                    group: mapping.group,
                    role: mapping.role,
                })
                .collect(),
            create_accounts: input.create_accounts,
            enabled: input.enabled,
        }
    }
}

#[derive(Deserialize, IntoParams)]
pub(crate) struct ProviderPath {
    /// The provider's identifier.
    id: String,
}

/// Identity providers, without their secrets.
#[utoipa::path(get, path = "/api/v1/identity-providers", 
    responses((status = 200, body = Vec<IdentityProviderResponse>)), tag = "identity")]
pub(crate) async fn list_identity_providers<U>(
    State(state): State<ApiState<U>>,
) -> Result<Json<Vec<IdentityProviderResponse>>, ApiError> {
    let views = access(&state)?.directory.list().await?;
    Ok(Json(views.into_iter().map(Into::into).collect()))
}

/// One identity provider.
#[utoipa::path(get, path = "/api/v1/identity-providers/{id}", params(ProviderPath),
    responses((status = 200, body = IdentityProviderResponse)), tag = "identity")]
pub(crate) async fn get_identity_provider<U>(
    State(state): State<ApiState<U>>,
    Path(path): Path<ProviderPath>,
) -> Result<Json<IdentityProviderResponse>, ApiError> {
    Ok(Json(access(&state)?.directory.get(&path.id).await?.into()))
}

/// Creates or replaces an identity provider; an enabled one is checked
/// against the provider first.
#[utoipa::path(put, path = "/api/v1/identity-providers/{id}", request_body = IdentityProviderInput,
    params(ProviderPath),
    responses((status = 200, body = IdentityProviderResponse), (status = 201, body = IdentityProviderResponse)),
    tag = "identity")]
pub(crate) async fn put_identity_provider<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Extension(principal): Extension<Principal>,
    Path(path): Path<ProviderPath>,
    payload: Result<Json<IdentityProviderInput>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(input) = payload.map_err(ApiError::from_json)?;
    let (view, created) = access(&state)?
        .directory
        .put(
            &path.id,
            input.into(),
            &request_scope(&headers)?,
            principal.actor(),
        )
        .await?;
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(IdentityProviderResponse::from(view))).into_response())
}

/// Deletes an identity provider and its links; sessions signed in through
/// it end, and the accounts stay.
#[utoipa::path(delete, path = "/api/v1/identity-providers/{id}", params(ProviderPath),
    responses((status = 204)), tag = "identity")]
pub(crate) async fn delete_identity_provider<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Extension(principal): Extension<Principal>,
    Path(path): Path<ProviderPath>,
) -> Result<StatusCode, ApiError> {
    access(&state)?
        .directory
        .delete(&path.id, &request_scope(&headers)?, principal.actor())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A provider people may sign in with.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct SignInOptionResponse {
    pub id: String,
    pub display_name: String,
}

/// The providers the sign-in page offers.
#[utoipa::path(get, path = "/api/v1/auth/providers",
    responses((status = 200, body = Vec<SignInOptionResponse>)), tag = "identity")]
pub(crate) async fn sign_in_options<U>(
    State(state): State<ApiState<U>>,
) -> Result<Json<Vec<SignInOptionResponse>>, ApiError> {
    let Some(sign_ins) = state
        .providers
        .as_ref()
        .and_then(|access| access.sign_ins.clone())
    else {
        return Ok(Json(Vec::new()));
    };
    Ok(Json(
        sign_ins
            .options()
            .await?
            .into_iter()
            .map(|option| SignInOptionResponse {
                id: option.id,
                display_name: option.display_name,
            })
            .collect(),
    ))
}

fn sign_ins<U>(state: &ApiState<U>) -> Result<ProviderSignIns, ApiError> {
    access(state)?.sign_ins.clone().ok_or_else(|| {
        ApiError::new(PanelError::unavailable(
            "signing in through providers needs the panel's public origin",
        ))
    })
}

#[derive(Deserialize, IntoParams)]
pub(crate) struct StartQuery {
    /// The panel path to return to.
    return_to: Option<String>,
}

fn see_other(location: &str) -> Result<Response, ApiError> {
    let location = HeaderValue::from_str(location)
        .map_err(|_| ApiError::new(PanelError::internal("the redirect is not a header")))?;
    Ok((StatusCode::SEE_OTHER, [(header::LOCATION, location)]).into_response())
}

fn sign_in_cookie(state: Option<&str>) -> HeaderValue {
    let (value, max_age) = match state {
        Some(state) => (state, SIGN_IN_COOKIE_SECONDS),
        None => ("", 0),
    };
    HeaderValue::from_str(&format!(
        "{SIGN_IN_COOKIE}={value}; Path=/api/v1/auth/oidc; Secure; HttpOnly; SameSite=Lax; Max-Age={max_age}"
    ))
    .expect("sign-in states are header-safe")
}

/// Sends the browser to the provider to sign in.
#[utoipa::path(get, path = "/api/v1/auth/oidc/{id}/start", params(ProviderPath, StartQuery),
    responses((status = 303, description = "To the provider")), tag = "identity")]
pub(crate) async fn start_sign_in<U>(
    State(state): State<ApiState<U>>,
    Path(path): Path<ProviderPath>,
    Query(query): Query<StartQuery>,
) -> Result<Response, ApiError> {
    let started = sign_ins(&state)?
        .start(&path.id, query.return_to.as_deref())
        .await?;
    let mut response = see_other(&started.url)?;
    response
        .headers_mut()
        .insert(header::SET_COOKIE, sign_in_cookie(Some(&started.state)));
    Ok(response)
}

#[derive(Deserialize, IntoParams)]
pub(crate) struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    /// Set by the provider when the sign-in did not happen.
    error: Option<String>,
    error_description: Option<String>,
}

/// Completes a sign-in when the provider sends the browser back, and opens a
/// session. Failures return to the sign-in page with the error code alone,
/// so that nobody can make the page say something of their choosing.
#[utoipa::path(get, path = "/api/v1/auth/oidc/{id}/callback", params(ProviderPath, CallbackQuery),
    responses((status = 303, description = "To the console")), tag = "identity")]
pub(crate) async fn finish_sign_in<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    peer: Option<Extension<ConnectInfo<SocketAddr>>>,
    Path(path): Path<ProviderPath>,
    Query(query): Query<CallbackQuery>,
) -> Result<Response, ApiError> {
    let browser_state = cookie(&headers, SIGN_IN_COOKIE)
        .unwrap_or_default()
        .to_owned();
    let outcome = async {
        if let Some(error) = &query.error {
            return Err(ApiError::new(PanelError::permission_denied(format!(
                "the provider did not sign you in: {}",
                query.error_description.as_deref().unwrap_or(error)
            ))));
        }
        let (Some(code), Some(returned_state)) = (&query.code, &query.state) else {
            return Err(ApiError::new(PanelError::invalid_argument(
                "the provider sent no code or state",
            )));
        };
        let gate = state.identity.clone().ok_or_else(|| {
            ApiError::new(PanelError::unavailable("accounts are not available here"))
        })?;
        let client = client(
            &headers,
            peer.map(|Extension(ConnectInfo(address))| address),
        );
        gate.admit_login(&client)?;
        let (login, return_to) = sign_ins(&state)?
            .finish(
                &path.id,
                code,
                returned_state,
                &browser_state,
                Transport::Cookie,
                &client,
                &request_scope(&headers)?,
            )
            .await?;
        Ok((login, return_to, gate.identity.settings().sessions.absolute))
    }
    .await;
    let mut response = match outcome {
        Ok((login, return_to, lifetime)) => {
            let mut response = see_other(&return_to)?;
            response.headers_mut().append(
                header::SET_COOKIE,
                session_cookie(Some(login.secret.expose()), lifetime),
            );
            response
        }
        Err(error) => see_other(&format!(
            "/login?sign_in_error={}",
            error.code().to_ascii_lowercase()
        ))?,
    };
    response
        .headers_mut()
        .append(header::SET_COOKIE, sign_in_cookie(None));
    Ok(response)
}

/// Who may sign in with a password.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum PasswordSignInMode {
    Everyone,
    /// Everyone else signs in through an identity provider.
    BreakGlassOnly,
}

/// How people may sign in.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct SignInPolicy {
    pub password_sign_in: PasswordSignInMode,
}

impl From<PasswordSignIn> for SignInPolicy {
    fn from(policy: PasswordSignIn) -> Self {
        Self {
            password_sign_in: match policy {
                PasswordSignIn::BreakGlassOnly => PasswordSignInMode::BreakGlassOnly,
                _ => PasswordSignInMode::Everyone,
            },
        }
    }
}

/// How people may sign in.
#[utoipa::path(get, path = "/api/v1/sign-in-policy",
    responses((status = 200, body = SignInPolicy)), tag = "identity")]
pub(crate) async fn get_sign_in_policy<U>(
    State(state): State<ApiState<U>>,
) -> Result<Json<SignInPolicy>, ApiError> {
    Ok(Json(
        access(&state)?.directory.password_sign_in().await?.into(),
    ))
}

/// Limits password sign-in to break-glass accounts, which needs an enabled
/// identity provider and an enabled break-glass account that can manage
/// accounts, or opens it to everyone again.
#[utoipa::path(put, path = "/api/v1/sign-in-policy", request_body = SignInPolicy,
    responses((status = 200, body = SignInPolicy)), tag = "identity")]
pub(crate) async fn put_sign_in_policy<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Extension(principal): Extension<Principal>,
    payload: Result<Json<SignInPolicy>, JsonRejection>,
) -> Result<Json<SignInPolicy>, ApiError> {
    let Json(policy) = payload.map_err(ApiError::from_json)?;
    let policy = match policy.password_sign_in {
        PasswordSignInMode::Everyone => PasswordSignIn::Everyone,
        PasswordSignInMode::BreakGlassOnly => PasswordSignIn::BreakGlassOnly,
    };
    access(&state)?
        .directory
        .set_password_sign_in(policy, &request_scope(&headers)?, principal.actor())
        .await?;
    Ok(Json(policy.into()))
}
