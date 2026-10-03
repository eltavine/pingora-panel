//! Setup, login sessions, the caller's own account, and account
//! administration. These contract types are the API's own; the identity
//! rules behind them may change without changing them.

use crate::{
    access::{client, session_cookie, Gate},
    error::ApiError,
    request_context::request_scope,
    ApiState,
};
use axum::{
    extract::{rejection::JsonRejection, ConnectInfo, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use chrono::{DateTime, Utc};
use panel_errors::PanelError;
use panel_identity::{
    Account, AccountChange, AccountId, AccountRequest, ApiToken, Credential, Permission,
    PermissionSet, Principal, Role, RoleRequest, Session, SessionId, TokenId, TokenRequest,
    Transport,
};
use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

/// An account.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AccountView {
    pub id: Uuid,
    pub username: String,
    pub display_name: Option<String>,
    /// Disabled accounts cannot log in and their tokens stop working.
    pub disabled: bool,
    /// Too many failed logins locked the password until it is unlocked.
    pub locked: bool,
    pub roles: Vec<String>,
    /// Keeps password sign-in when it is limited to break-glass accounts;
    /// every sign-in with it is recorded as `identity.break_glass.used`.
    pub break_glass: bool,
    /// Belongs to a program; it never signs in.
    pub service: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
    pub password_changed_at: Option<DateTime<Utc>>,
}

impl From<Account> for AccountView {
    fn from(account: Account) -> Self {
        Self {
            id: account.id.as_uuid(),
            username: account.username.to_string(),
            display_name: account.display_name,
            disabled: account.disabled,
            locked: account.locked,
            roles: account.roles,
            break_glass: account.break_glass,
            service: account.service,
            created_at: account.created_at,
            updated_at: account.updated_at,
            last_login_at: account.last_login_at,
            password_changed_at: account.password_changed_at,
        }
    }
}

/// A permission of the catalog.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct PermissionView {
    pub name: String,
    pub description: String,
}

/// A named set of permissions.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct RoleView {
    pub id: String,
    pub name: String,
    pub description: String,
    pub permissions: Vec<String>,
    /// Shipped with the panel.
    pub built_in: bool,
}

impl From<Role> for RoleView {
    fn from(role: Role) -> Self {
        Self {
            permissions: names(&role.permissions),
            id: role.id,
            name: role.name,
            description: role.description,
            built_in: role.built_in,
        }
    }
}

fn names(permissions: &PermissionSet) -> Vec<String> {
    permissions.names().into_iter().map(str::to_owned).collect()
}

/// How a session's secret travels.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SessionTransport {
    /// A `Secure`, `HttpOnly`, `SameSite=Strict` cookie, for browsers.
    #[default]
    Cookie,
    /// Returned once in the response and sent as `Authorization: Bearer`.
    Bearer,
}

/// A login session.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct SessionView {
    pub id: Uuid,
    pub transport: SessionTransport,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    /// When it ends unless used before.
    pub idle_until: DateTime<Utc>,
    /// When it ends regardless of use.
    pub expires_at: DateTime<Utc>,
    pub client_address: Option<String>,
    pub user_agent: Option<String>,
    /// Whether it is the session of the request.
    pub current: bool,
}

fn session_view(session: &Session, gate: &Gate, current: Option<SessionId>) -> SessionView {
    SessionView {
        id: session.id.as_uuid(),
        transport: match session.transport {
            Transport::Bearer => SessionTransport::Bearer,
            _ => SessionTransport::Cookie,
        },
        created_at: session.created_at,
        last_seen_at: session.last_seen_at,
        idle_until: session.idle_until(&gate.identity.settings().sessions),
        expires_at: session.expires_at,
        client_address: session.client_address.clone(),
        user_agent: session.user_agent.clone(),
        current: current == Some(session.id),
    }
}

/// An API token, without its secret.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct TokenView {
    pub id: Uuid,
    pub name: String,
    pub permissions: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl From<ApiToken> for TokenView {
    fn from(token: ApiToken) -> Self {
        Self {
            id: token.id.as_uuid(),
            permissions: names(&token.permissions),
            name: token.name,
            created_at: token.created_at,
            expires_at: token.expires_at,
            last_used_at: token.last_used_at,
            revoked_at: token.revoked_at,
        }
    }
}

/// How the request authenticated.
#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CredentialKind {
    Cookie,
    Bearer,
    Token,
}

/// The caller and what it may do.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct CurrentSession {
    pub account: AccountView,
    pub permissions: Vec<String>,
    pub credential: CredentialKind,
    /// The login session, unless an API token authenticated the request.
    pub session: Option<SessionView>,
    /// Sent in `x-csrf-token` with every unsafe request of a cookie session.
    pub csrf_token: Option<String>,
}

/// A new session.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct LoginResponse {
    #[serde(flatten)]
    pub current: CurrentSession,
    /// The session secret, for bearer sessions only; it is not shown again.
    pub secret: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub transport: SessionTransport,
}

/// Whether the first account still has to be created.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct SetupStatus {
    pub required: bool,
}

/// The first account, created with the deployment's one-time bootstrap
/// token.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct SetupRequest {
    pub token: String,
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct PasswordChange {
    pub current: String,
    pub new: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct PasswordReset {
    pub password: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct NewAccount {
    pub username: String,
    #[serde(default)]
    pub display_name: Option<String>,
    /// Without one the account cannot log in until a password is set.
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
    /// A service account for a program: no password, and account managers
    /// issue its API tokens.
    #[serde(default)]
    pub service: bool,
}

/// Changes to an account; absent fields stay as they are.
#[derive(Clone, Debug, Default, Deserialize, Serialize, ToSchema)]
pub struct AccountPatch {
    /// An empty name removes it.
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub disabled: Option<bool>,
    #[serde(default)]
    pub roles: Option<Vec<String>>,
    #[serde(default)]
    pub break_glass: Option<bool>,
    /// Clears failed logins and re-enables a locked password.
    #[serde(default)]
    pub unlock: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct NewToken {
    pub name: String,
    /// The caller's permissions when absent; never more than those.
    #[serde(default)]
    pub permissions: Option<Vec<String>>,
    /// Between 1 and 365.
    pub expires_in_days: u32,
}

/// A new API token and its secret, which is not shown again.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct CreatedToken {
    pub token: TokenView,
    pub secret: String,
}

/// A role of the caller's choosing.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct NewRole {
    /// 1 to 64 lowercase letters, digits, `.`, `_` or `-`.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub permissions: Vec<String>,
}

/// What a role that is not built in becomes.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
pub struct RoleChange {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub permissions: Vec<String>,
}

/// How many sessions ended.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct EndedSessions {
    pub ended: u64,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct RolePath {
    /// Role identifier.
    id: String,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct AccountPath {
    /// Account identifier.
    id: Uuid,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct OwnItemPath {
    /// Session or token identifier.
    id: Uuid,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct AccountSessionPath {
    /// Account identifier.
    id: Uuid,
    /// Session identifier.
    session: Uuid,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct AccountTokenPath {
    /// Account identifier.
    id: Uuid,
    /// Token identifier.
    token: Uuid,
}

type Peer = Option<Extension<ConnectInfo<SocketAddr>>>;

pub(crate) fn gate<U>(state: &ApiState<U>) -> Result<Arc<Gate>, ApiError> {
    state.identity.clone().ok_or_else(|| {
        ApiError::new(PanelError::unavailable(
            "this API does not authenticate callers",
        ))
    })
}

fn body<T>(payload: Result<Json<T>, JsonRejection>) -> Result<T, ApiError> {
    payload
        .map(|Json(value)| value)
        .map_err(ApiError::from_json)
}

async fn current(gate: &Gate, principal: &Principal) -> Result<CurrentSession, ApiError> {
    let account = gate.identity.account(principal.account).await?;
    let session = match principal.session() {
        Some(id) => gate
            .identity
            .sessions(principal.account)
            .await?
            .iter()
            .find(|session| session.id == id)
            .map(|session| session_view(session, gate, Some(id))),
        None => None,
    };
    Ok(CurrentSession {
        account: account.into(),
        permissions: names(&principal.permissions),
        credential: match principal.credential {
            Credential::SessionCookie { .. } => CredentialKind::Cookie,
            Credential::SessionBearer { .. } => CredentialKind::Bearer,
            _ => CredentialKind::Token,
        },
        session,
        csrf_token: principal.csrf_token().map(str::to_owned),
    })
}

/// Whether the first account still has to be created.
#[utoipa::path(get, path = "/api/v1/setup", responses((status = 200, body = SetupStatus)), tag = "identity")]
pub(crate) async fn setup_status<U>(
    State(state): State<ApiState<U>>,
) -> Result<Json<SetupStatus>, ApiError> {
    let gate = gate(&state)?;
    Ok(Json(SetupStatus {
        required: gate.identity.setup_required().await?,
    }))
}

/// Creates the first account, an Administrator, with the one-time
/// bootstrap token; refused once any account exists.
#[utoipa::path(post, path = "/api/v1/setup", request_body = SetupRequest,
    responses((status = 201, body = AccountView)), tag = "identity")]
pub(crate) async fn setup<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    peer: Peer,
    payload: Result<Json<SetupRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let gate = gate(&state)?;
    gate.admit_login(&client(&headers, peer.map(|Extension(ConnectInfo(a))| a)))?;
    let request = body(payload)?;
    let account = gate
        .identity
        .setup(
            &request.token,
            AccountRequest {
                username: request.username,
                display_name: request.display_name,
                password: Some(request.password),
                roles: Vec::new(),
                service: false,
            },
            &request_scope(&headers)?,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(AccountView::from(account))).into_response())
}

/// Logs in. Browsers get the session as a cookie; `bearer` returns its
/// secret once instead.
#[utoipa::path(post, path = "/api/v1/session", request_body = LoginRequest,
    responses((status = 201, body = LoginResponse, headers(("Set-Cookie" = String)))), tag = "identity")]
pub(crate) async fn login<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    peer: Peer,
    payload: Result<Json<LoginRequest>, JsonRejection>,
) -> Result<Response, ApiError> {
    let gate = gate(&state)?;
    let client = client(&headers, peer.map(|Extension(ConnectInfo(a))| a));
    gate.admit_login(&client)?;
    let request = body(payload)?;
    let transport = match request.transport {
        SessionTransport::Bearer => Transport::Bearer,
        SessionTransport::Cookie => Transport::Cookie,
    };
    let login = gate
        .identity
        .login(
            &request.username,
            &request.password,
            transport,
            &client,
            &request_scope(&headers)?,
        )
        .await?;
    let principal = gate
        .identity
        .authenticate_session(login.secret.expose(), transport)
        .await?
        .ok_or_else(|| PanelError::internal("the new session is not live"))?;
    let current = current(&gate, &principal).await?;
    let lifetime = gate.identity.settings().sessions.absolute;
    let mut response = (
        StatusCode::CREATED,
        Json(LoginResponse {
            current,
            secret: (transport == Transport::Bearer).then(|| login.secret.expose().to_owned()),
        }),
    )
        .into_response();
    if transport == Transport::Cookie {
        response.headers_mut().insert(
            header::SET_COOKIE,
            session_cookie(Some(login.secret.expose()), lifetime),
        );
    }
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().expect("static"));
    Ok(response)
}

/// The caller, its permissions and session.
#[utoipa::path(get, path = "/api/v1/session", responses((status = 200, body = CurrentSession)), tag = "identity")]
pub(crate) async fn session<U>(
    State(state): State<ApiState<U>>,
    Extension(principal): Extension<Principal>,
) -> Result<Response, ApiError> {
    let gate = gate(&state)?;
    let mut response = Json(current(&gate, &principal).await?).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().expect("static"));
    Ok(response)
}

/// Logs out: ends the caller's session.
#[utoipa::path(delete, path = "/api/v1/session", responses((status = 204)), tag = "identity")]
pub(crate) async fn logout<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Extension(principal): Extension<Principal>,
) -> Result<Response, ApiError> {
    let gate = gate(&state)?;
    gate.identity
        .logout(&principal, &request_scope(&headers)?)
        .await?;
    let mut response = StatusCode::NO_CONTENT.into_response();
    if principal.csrf_token().is_some() {
        response
            .headers_mut()
            .insert(header::SET_COOKIE, session_cookie(None, Duration::ZERO));
    }
    Ok(response)
}

/// The permission catalog.
#[utoipa::path(get, path = "/api/v1/permissions", responses((status = 200, body = [PermissionView])), tag = "identity")]
pub(crate) async fn permissions() -> Json<Vec<PermissionView>> {
    Json(
        Permission::all()
            .map(|permission| PermissionView {
                name: permission.name().into(),
                description: permission.description().into(),
            })
            .collect(),
    )
}

/// Changes the caller's password; their other sessions end.
#[utoipa::path(put, path = "/api/v1/account/password", request_body = PasswordChange,
    responses((status = 204)), tag = "identity")]
pub(crate) async fn change_password<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Extension(principal): Extension<Principal>,
    payload: Result<Json<PasswordChange>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let gate = gate(&state)?;
    let request = body(payload)?;
    gate.identity
        .change_password(
            &principal,
            &request.current,
            &request.new,
            &request_scope(&headers)?,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The caller's sessions that have not ended.
#[utoipa::path(get, path = "/api/v1/account/sessions", responses((status = 200, body = [SessionView])), tag = "identity")]
pub(crate) async fn own_sessions<U>(
    State(state): State<ApiState<U>>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<SessionView>>, ApiError> {
    let gate = gate(&state)?;
    let sessions = gate.identity.sessions(principal.account).await?;
    Ok(Json(
        sessions
            .iter()
            .map(|session| session_view(session, &gate, principal.session()))
            .collect(),
    ))
}

/// Ends one of the caller's sessions.
#[utoipa::path(delete, path = "/api/v1/account/sessions/{id}", params(OwnItemPath),
    responses((status = 204)), tag = "identity")]
pub(crate) async fn end_own_session<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Extension(principal): Extension<Principal>,
    Path(path): Path<OwnItemPath>,
) -> Result<StatusCode, ApiError> {
    let gate = gate(&state)?;
    gate.identity
        .end_session(
            principal.account,
            SessionId::from_uuid(path.id),
            &request_scope(&headers)?,
            principal.actor(),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The caller's API tokens.
#[utoipa::path(get, path = "/api/v1/account/tokens", responses((status = 200, body = [TokenView])), tag = "identity")]
pub(crate) async fn own_tokens<U>(
    State(state): State<ApiState<U>>,
    Extension(principal): Extension<Principal>,
) -> Result<Json<Vec<TokenView>>, ApiError> {
    let gate = gate(&state)?;
    let tokens = gate.identity.tokens(principal.account).await?;
    Ok(Json(tokens.into_iter().map(TokenView::from).collect()))
}

/// Creates an API token for the caller. Its secret is returned once.
#[utoipa::path(post, path = "/api/v1/account/tokens", request_body = NewToken,
    responses((status = 201, body = CreatedToken)), tag = "identity")]
pub(crate) async fn create_token<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Extension(principal): Extension<Principal>,
    payload: Result<Json<NewToken>, JsonRejection>,
) -> Result<Response, ApiError> {
    let gate = gate(&state)?;
    let request = body(payload)?;
    let permissions = request
        .permissions
        .as_deref()
        .map(PermissionSet::from_names)
        .transpose()?;
    let (token, secret) = gate
        .identity
        .create_token(
            &principal,
            TokenRequest {
                name: request.name,
                permissions,
                lifetime: Duration::from_secs(u64::from(request.expires_in_days) * 86_400),
            },
            &request_scope(&headers)?,
        )
        .await?;
    let mut response = (
        StatusCode::CREATED,
        Json(CreatedToken {
            token: token.into(),
            secret: secret.expose().to_owned(),
        }),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().expect("static"));
    Ok(response)
}

/// Revokes one of the caller's API tokens.
#[utoipa::path(delete, path = "/api/v1/account/tokens/{id}", params(OwnItemPath),
    responses((status = 204)), tag = "identity")]
pub(crate) async fn revoke_own_token<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Extension(principal): Extension<Principal>,
    Path(path): Path<OwnItemPath>,
) -> Result<StatusCode, ApiError> {
    let gate = gate(&state)?;
    gate.identity
        .revoke_token(
            principal.account,
            TokenId::from_uuid(path.id),
            &request_scope(&headers)?,
            principal.actor(),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Every account.
#[utoipa::path(get, path = "/api/v1/accounts", responses((status = 200, body = [AccountView])), tag = "identity")]
pub(crate) async fn list_accounts<U>(
    State(state): State<ApiState<U>>,
) -> Result<Json<Vec<AccountView>>, ApiError> {
    let gate = gate(&state)?;
    let accounts = gate.identity.accounts().await?;
    Ok(Json(accounts.into_iter().map(AccountView::from).collect()))
}

/// Creates an account.
#[utoipa::path(post, path = "/api/v1/accounts", request_body = NewAccount,
    responses((status = 201, body = AccountView)), tag = "identity")]
pub(crate) async fn create_account<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Extension(principal): Extension<Principal>,
    payload: Result<Json<NewAccount>, JsonRejection>,
) -> Result<Response, ApiError> {
    let gate = gate(&state)?;
    let request = body(payload)?;
    let account = gate
        .identity
        .create_account(
            AccountRequest {
                username: request.username,
                display_name: request.display_name.filter(|name| !name.trim().is_empty()),
                password: request.password,
                roles: request.roles,
                service: request.service,
            },
            &request_scope(&headers)?,
            principal.actor(),
        )
        .await?;
    Ok((StatusCode::CREATED, Json(AccountView::from(account))).into_response())
}

/// One account.
#[utoipa::path(get, path = "/api/v1/accounts/{id}", params(AccountPath),
    responses((status = 200, body = AccountView)), tag = "identity")]
pub(crate) async fn get_account<U>(
    State(state): State<ApiState<U>>,
    Path(path): Path<AccountPath>,
) -> Result<Json<AccountView>, ApiError> {
    let gate = gate(&state)?;
    let account = gate.identity.account(AccountId::from_uuid(path.id)).await?;
    Ok(Json(account.into()))
}

/// Changes an account: its name, roles, whether it is disabled, or
/// unlocks its password. At least one enabled account must remain able to
/// manage accounts.
#[utoipa::path(patch, path = "/api/v1/accounts/{id}", params(AccountPath), request_body = AccountPatch,
    responses((status = 200, body = AccountView)), tag = "identity")]
pub(crate) async fn update_account<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Extension(principal): Extension<Principal>,
    Path(path): Path<AccountPath>,
    payload: Result<Json<AccountPatch>, JsonRejection>,
) -> Result<Json<AccountView>, ApiError> {
    let gate = gate(&state)?;
    let request = body(payload)?;
    let account = gate
        .identity
        .update_account(
            AccountId::from_uuid(path.id),
            AccountChange {
                display_name: request
                    .display_name
                    .map(|name| Some(name).filter(|name| !name.trim().is_empty())),
                disabled: request.disabled,
                roles: request.roles,
                break_glass: request.break_glass,
                unlock: request.unlock,
            },
            &request_scope(&headers)?,
            principal.actor(),
        )
        .await?;
    Ok(Json(account.into()))
}

/// Sets an account's password; all of its sessions end.
#[utoipa::path(put, path = "/api/v1/accounts/{id}/password", params(AccountPath), request_body = PasswordReset,
    responses((status = 204)), tag = "identity")]
pub(crate) async fn reset_password<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Extension(principal): Extension<Principal>,
    Path(path): Path<AccountPath>,
    payload: Result<Json<PasswordReset>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let gate = gate(&state)?;
    let request = body(payload)?;
    gate.identity
        .reset_password(
            AccountId::from_uuid(path.id),
            &request.password,
            &request_scope(&headers)?,
            principal.actor(),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// An account's sessions that have not ended.
#[utoipa::path(get, path = "/api/v1/accounts/{id}/sessions", params(AccountPath),
    responses((status = 200, body = [SessionView])), tag = "identity")]
pub(crate) async fn account_sessions<U>(
    State(state): State<ApiState<U>>,
    Extension(principal): Extension<Principal>,
    Path(path): Path<AccountPath>,
) -> Result<Json<Vec<SessionView>>, ApiError> {
    let gate = gate(&state)?;
    let sessions = gate
        .identity
        .sessions(AccountId::from_uuid(path.id))
        .await?;
    Ok(Json(
        sessions
            .iter()
            .map(|session| session_view(session, &gate, principal.session()))
            .collect(),
    ))
}

/// Ends a session of an account.
#[utoipa::path(delete, path = "/api/v1/accounts/{id}/sessions/{session}", params(AccountSessionPath),
    responses((status = 204)), tag = "identity")]
pub(crate) async fn end_account_session<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Extension(principal): Extension<Principal>,
    Path(path): Path<AccountSessionPath>,
) -> Result<StatusCode, ApiError> {
    let gate = gate(&state)?;
    gate.identity
        .end_session(
            AccountId::from_uuid(path.id),
            SessionId::from_uuid(path.session),
            &request_scope(&headers)?,
            principal.actor(),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Issues an API token for a service account; its secret is returned once.
#[utoipa::path(post, path = "/api/v1/accounts/{id}/tokens", params(AccountPath),
    request_body = NewToken, responses((status = 201, body = CreatedToken)), tag = "identity")]
pub(crate) async fn issue_token<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Extension(principal): Extension<Principal>,
    Path(path): Path<AccountPath>,
    payload: Result<Json<NewToken>, JsonRejection>,
) -> Result<Response, ApiError> {
    let gate = gate(&state)?;
    let request = body(payload)?;
    let permissions = request
        .permissions
        .as_deref()
        .map(PermissionSet::from_names)
        .transpose()?;
    let (token, secret) = gate
        .identity
        .issue_token(
            AccountId::from_uuid(path.id),
            TokenRequest {
                name: request.name,
                permissions,
                lifetime: Duration::from_secs(u64::from(request.expires_in_days) * 86_400),
            },
            &request_scope(&headers)?,
            principal.actor(),
        )
        .await?;
    let mut response = (
        StatusCode::CREATED,
        Json(CreatedToken {
            token: token.into(),
            secret: secret.expose().to_owned(),
        }),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().expect("static"));
    Ok(response)
}

/// An account's API tokens.
#[utoipa::path(get, path = "/api/v1/accounts/{id}/tokens", params(AccountPath),
    responses((status = 200, body = [TokenView])), tag = "identity")]
pub(crate) async fn account_tokens<U>(
    State(state): State<ApiState<U>>,
    Path(path): Path<AccountPath>,
) -> Result<Json<Vec<TokenView>>, ApiError> {
    let gate = gate(&state)?;
    let tokens = gate.identity.tokens(AccountId::from_uuid(path.id)).await?;
    Ok(Json(tokens.into_iter().map(TokenView::from).collect()))
}

/// Revokes an API token of an account.
#[utoipa::path(delete, path = "/api/v1/accounts/{id}/tokens/{token}", params(AccountTokenPath),
    responses((status = 204)), tag = "identity")]
pub(crate) async fn revoke_account_token<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Extension(principal): Extension<Principal>,
    Path(path): Path<AccountTokenPath>,
) -> Result<StatusCode, ApiError> {
    let gate = gate(&state)?;
    gate.identity
        .revoke_token(
            AccountId::from_uuid(path.id),
            TokenId::from_uuid(path.token),
            &request_scope(&headers)?,
            principal.actor(),
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Every role.
#[utoipa::path(get, path = "/api/v1/roles", responses((status = 200, body = [RoleView])), tag = "identity")]
pub(crate) async fn list_roles<U>(
    State(state): State<ApiState<U>>,
) -> Result<Json<Vec<RoleView>>, ApiError> {
    let gate = gate(&state)?;
    let roles = gate.identity.roles().await?;
    Ok(Json(roles.into_iter().map(RoleView::from).collect()))
}

/// Creates a role.
#[utoipa::path(post, path = "/api/v1/roles", request_body = NewRole,
    responses((status = 201, body = RoleView)), tag = "identity")]
pub(crate) async fn create_role<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Extension(principal): Extension<Principal>,
    payload: Result<Json<NewRole>, JsonRejection>,
) -> Result<Response, ApiError> {
    let gate = gate(&state)?;
    let request = body(payload)?;
    let role = gate
        .identity
        .create_role(
            RoleRequest {
                id: request.id,
                name: request.name,
                description: request.description,
                permissions: PermissionSet::from_names(&request.permissions)?,
            },
            &request_scope(&headers)?,
            principal.actor(),
        )
        .await?;
    Ok((StatusCode::CREATED, Json(RoleView::from(role))).into_response())
}

/// Replaces a role that is not built in.
#[utoipa::path(put, path = "/api/v1/roles/{id}", params(RolePath), request_body = RoleChange,
    responses((status = 200, body = RoleView)), tag = "identity")]
pub(crate) async fn replace_role<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Extension(principal): Extension<Principal>,
    Path(path): Path<RolePath>,
    payload: Result<Json<RoleChange>, JsonRejection>,
) -> Result<Json<RoleView>, ApiError> {
    let gate = gate(&state)?;
    let request = body(payload)?;
    let role = gate
        .identity
        .update_role(
            RoleRequest {
                id: path.id,
                name: request.name,
                description: request.description,
                permissions: PermissionSet::from_names(&request.permissions)?,
            },
            &request_scope(&headers)?,
            principal.actor(),
        )
        .await?;
    Ok(Json(role.into()))
}

/// Deletes a role that is not built in and that no account holds.
#[utoipa::path(delete, path = "/api/v1/roles/{id}", params(RolePath),
    responses((status = 204)), tag = "identity")]
pub(crate) async fn delete_role<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Extension(principal): Extension<Principal>,
    Path(path): Path<RolePath>,
) -> Result<StatusCode, ApiError> {
    let gate = gate(&state)?;
    gate.identity
        .delete_role(&path.id, &request_scope(&headers)?, principal.actor())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Ends every session of the caller but the one making the request.
#[utoipa::path(delete, path = "/api/v1/account/sessions",
    responses((status = 200, body = EndedSessions)), tag = "identity")]
pub(crate) async fn end_other_sessions<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Extension(principal): Extension<Principal>,
) -> Result<Json<EndedSessions>, ApiError> {
    let gate = gate(&state)?;
    let ended = gate
        .identity
        .end_sessions(
            principal.account,
            principal.session(),
            &request_scope(&headers)?,
            principal.actor(),
        )
        .await?;
    Ok(Json(EndedSessions { ended }))
}

/// Ends every session of an account.
#[utoipa::path(delete, path = "/api/v1/accounts/{id}/sessions", params(AccountPath),
    responses((status = 200, body = EndedSessions)), tag = "identity")]
pub(crate) async fn end_account_sessions<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Extension(principal): Extension<Principal>,
    Path(path): Path<AccountPath>,
) -> Result<Json<EndedSessions>, ApiError> {
    let gate = gate(&state)?;
    let ended = gate
        .identity
        .end_sessions(
            AccountId::from_uuid(path.id),
            None,
            &request_scope(&headers)?,
            principal.actor(),
        )
        .await?;
    Ok(Json(EndedSessions { ended }))
}

/// Replaces one of the caller's API tokens by one with a new secret, the
/// same name and permissions and a fresh lifetime of the same length; the
/// old secret stops working at once.
#[utoipa::path(post, path = "/api/v1/account/tokens/{id}/rotate", params(OwnItemPath),
    responses((status = 201, body = CreatedToken)), tag = "identity")]
pub(crate) async fn rotate_token<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Extension(principal): Extension<Principal>,
    Path(path): Path<OwnItemPath>,
) -> Result<Response, ApiError> {
    let gate = gate(&state)?;
    let (token, secret) = gate
        .identity
        .rotate_token(
            &principal,
            TokenId::from_uuid(path.id),
            &request_scope(&headers)?,
        )
        .await?;
    let mut response = (
        StatusCode::CREATED,
        Json(CreatedToken {
            token: token.into(),
            secret: secret.expose().to_owned(),
        }),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().expect("static"));
    Ok(response)
}
