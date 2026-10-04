//! Who may call each route. Every route declares its access in one table;
//! the guard authenticates the caller, refuses cross-site and unverified
//! requests of cookie sessions, checks the permission and records the
//! caller as the request's actor. A route without an entry is refused.

use crate::{error::ApiError, request_context::request_scope, ApiState};
use async_trait::async_trait;
use axum::extract::ConnectInfo;
use axum::{
    extract::{MatchedPath, Request, State},
    http::{header, HeaderMap, HeaderValue, Method},
    middleware::Next,
    response::{IntoResponse, Response},
};
use chrono::Utc;
use cookie::{Cookie, SameSite};
use governor::{DefaultKeyedRateLimiter, Quota, RateLimiter};
use panel_application::RequestScope;
use panel_application::{SiteAccess, SiteScope};
use panel_errors::PanelError;
use panel_identity::{
    Access as HeldAccess, Client, GrantScope, Identity, Permission, Principal, Transport, SCOPABLE,
    TOKEN_PREFIX,
};
use std::{net::SocketAddr, num::NonZeroU32, sync::Arc, time::Duration};

pub(crate) const SESSION_COOKIE: &str = "__Host-ppanel_session";
pub(crate) const CSRF_HEADER: &str = "x-csrf-token";
pub(crate) const ACTOR_HEADER: &str = "x-actor";
/// The sites a request is limited to (ADR 0021); set only by the guard.
pub(crate) const SITE_SCOPE_HEADER: &str = "x-panel-site-scope";

/// The site scope of configuration permissions held only for some sites.
fn site_scope(access: &HeldAccess) -> SiteScope {
    let limited = SCOPABLE
        .iter()
        .filter_map(|permission| {
            let mut held = SiteAccess {
                permission: permission.name().into(),
                ..SiteAccess::default()
            };
            for scope in access.scopes(*permission) {
                match scope {
                    GrantScope::SiteGroup { group } => held.groups.push(group.clone()),
                    GrantScope::Site { site } => held.sites.push(site.to_string()),
                    _ => {}
                }
            }
            (!held.groups.is_empty() || !held.sites.is_empty()).then_some(held)
        })
        .collect();
    SiteScope {
        unrestricted: SCOPABLE
            .iter()
            .filter(|permission| access.unrestricted.contains(**permission))
            .map(|permission| permission.name().to_owned())
            .collect(),
        limited,
    }
}

/// What a route requires of its caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Access {
    /// Anyone: the API description, setup and login.
    Public,
    /// Any authenticated caller, for their own session and account.
    Signed,
    Requires(Permission),
}

use Access::{Public, Requires, Signed};
use Permission::*;

/// Every route with its access, by method and path template.
pub(crate) static ROUTES: &[(&str, &str, Access)] = &[
    ("GET", "/api/v1/openapi.json", Public),
    ("GET", "/api/v1/setup", Public),
    ("POST", "/api/v1/setup", Public),
    ("POST", "/api/v1/session", Public),
    ("GET", "/api/v1/session", Signed),
    ("DELETE", "/api/v1/session", Signed),
    ("GET", "/api/v1/permissions", Signed),
    ("PUT", "/api/v1/account/password", Signed),
    ("GET", "/api/v1/account/sessions", Signed),
    ("DELETE", "/api/v1/account/sessions", Signed),
    ("DELETE", "/api/v1/account/sessions/{id}", Signed),
    ("GET", "/api/v1/account/tokens", Signed),
    ("POST", "/api/v1/account/tokens", Signed),
    ("DELETE", "/api/v1/account/tokens/{id}", Signed),
    ("POST", "/api/v1/account/tokens/{id}/rotate", Signed),
    ("GET", "/api/v1/accounts", Requires(IdentityRead)),
    ("POST", "/api/v1/accounts", Requires(IdentityManage)),
    ("GET", "/api/v1/accounts/{id}", Requires(IdentityRead)),
    ("PATCH", "/api/v1/accounts/{id}", Requires(IdentityManage)),
    (
        "PUT",
        "/api/v1/accounts/{id}/password",
        Requires(IdentityManage),
    ),
    (
        "GET",
        "/api/v1/accounts/{id}/sessions",
        Requires(IdentityRead),
    ),
    (
        "DELETE",
        "/api/v1/accounts/{id}/sessions",
        Requires(IdentityManage),
    ),
    (
        "DELETE",
        "/api/v1/accounts/{id}/sessions/{session}",
        Requires(IdentityManage),
    ),
    (
        "GET",
        "/api/v1/accounts/{id}/tokens",
        Requires(IdentityRead),
    ),
    (
        "POST",
        "/api/v1/accounts/{id}/tokens",
        Requires(IdentityManage),
    ),
    (
        "GET",
        "/api/v1/accounts/{id}/grants",
        Requires(IdentityRead),
    ),
    (
        "POST",
        "/api/v1/accounts/{id}/grants",
        Requires(IdentityManage),
    ),
    (
        "DELETE",
        "/api/v1/accounts/{id}/grants/{grant}",
        Requires(IdentityManage),
    ),
    (
        "DELETE",
        "/api/v1/accounts/{id}/tokens/{token}",
        Requires(IdentityManage),
    ),
    ("GET", "/api/v1/roles", Requires(IdentityRead)),
    ("POST", "/api/v1/roles", Requires(IdentityManage)),
    ("PUT", "/api/v1/roles/{id}", Requires(IdentityManage)),
    ("DELETE", "/api/v1/roles/{id}", Requires(IdentityManage)),
    ("GET", "/api/v1/identity-providers", Requires(IdentityRead)),
    (
        "GET",
        "/api/v1/identity-providers/{id}",
        Requires(IdentityRead),
    ),
    (
        "PUT",
        "/api/v1/identity-providers/{id}",
        Requires(IdentityManage),
    ),
    (
        "DELETE",
        "/api/v1/identity-providers/{id}",
        Requires(IdentityManage),
    ),
    ("GET", "/api/v1/sign-in-policy", Requires(IdentityRead)),
    ("PUT", "/api/v1/sign-in-policy", Requires(IdentityManage)),
    ("GET", "/api/v1/workload-identities", Requires(IdentityRead)),
    (
        "GET",
        "/api/v1/workload-identities/{id}",
        Requires(IdentityRead),
    ),
    (
        "PUT",
        "/api/v1/workload-identities/{id}",
        Requires(IdentityManage),
    ),
    (
        "DELETE",
        "/api/v1/workload-identities/{id}",
        Requires(IdentityManage),
    ),
    ("POST", "/api/v1/auth/workload", Public),
    ("GET", "/api/v1/auth/providers", Public),
    ("GET", "/api/v1/auth/oidc/{id}/start", Public),
    ("GET", "/api/v1/auth/oidc/{id}/callback", Public),
    ("POST", "/api/v1/gateway/validate", Requires(GatewayPublish)),
    ("POST", "/api/v1/gateway/prepare", Requires(GatewayPublish)),
    ("POST", "/api/v1/gateway/activate", Requires(GatewayPublish)),
    ("POST", "/api/v1/gateway/abort", Requires(GatewayPublish)),
    ("GET", "/api/v1/gateway/status", Requires(GatewayRead)),
    (
        "GET",
        "/api/v1/gateway/receipts/{key}",
        Requires(GatewayRead),
    ),
    ("GET", "/api/v1/gateway/data-plane", Requires(GatewayRead)),
    ("GET", "/api/v1/gateway/file-checks", Requires(GatewayRead)),
    ("POST", "/api/v1/gateway/reload", Requires(GatewayOperate)),
    ("PUT", "/api/v1/gateway/workers", Requires(GatewayOperate)),
    ("POST", "/api/v1/gateway/shutdown", Requires(GatewayOperate)),
    ("GET", "/api/v1/upstreams/health", Requires(GatewayRead)),
    (
        "PUT",
        "/api/v1/upstreams/{id}/nodes/{node}/drain",
        Requires(GatewayOperate),
    ),
    (
        "DELETE",
        "/api/v1/upstreams/{id}/nodes/{node}/drain",
        Requires(GatewayOperate),
    ),
    ("GET", "/api/v1/platform/services", Requires(PlatformRead)),
    ("GET", "/api/v1/sites", Requires(ConfigRead)),
    ("POST", "/api/v1/sites", Requires(ConfigWrite)),
    ("GET", "/api/v1/sites/summary", Requires(ConfigRead)),
    ("GET", "/api/v1/sites/export", Requires(ConfigRead)),
    ("POST", "/api/v1/sites/import", Requires(ConfigWrite)),
    ("POST", "/api/v1/sites/batch", Requires(ConfigWrite)),
    ("GET", "/api/v1/sites/{id}", Requires(ConfigRead)),
    ("PUT", "/api/v1/sites/{id}", Requires(ConfigWrite)),
    ("DELETE", "/api/v1/sites/{id}", Requires(ConfigWrite)),
    ("POST", "/api/v1/sites/{id}/enable", Requires(ConfigWrite)),
    ("POST", "/api/v1/sites/{id}/disable", Requires(ConfigWrite)),
    ("POST", "/api/v1/sites/{id}/favorite", Requires(ConfigWrite)),
    (
        "POST",
        "/api/v1/sites/{id}/unfavorite",
        Requires(ConfigWrite),
    ),
    ("POST", "/api/v1/sites/{id}/restore", Requires(ConfigWrite)),
    ("POST", "/api/v1/sites/{id}/clone", Requires(ConfigWrite)),
    ("POST", "/api/v1/sites/{id}/domains", Requires(ConfigWrite)),
    (
        "PUT",
        "/api/v1/sites/{id}/domains/{host}",
        Requires(ConfigWrite),
    ),
    (
        "DELETE",
        "/api/v1/sites/{id}/domains/{host}",
        Requires(ConfigWrite),
    ),
    ("GET", "/api/v1/sites/{id}/routes", Requires(ConfigRead)),
    ("POST", "/api/v1/sites/{id}/routes", Requires(ConfigWrite)),
    (
        "PUT",
        "/api/v1/sites/{id}/routes/order",
        Requires(ConfigWrite),
    ),
    ("GET", "/api/v1/routes/{id}", Requires(ConfigRead)),
    ("PUT", "/api/v1/routes/{id}", Requires(ConfigWrite)),
    ("DELETE", "/api/v1/routes/{id}", Requires(ConfigWrite)),
    ("GET", "/api/v1/domains", Requires(ConfigRead)),
    ("POST", "/api/v1/domains/check", Requires(ConfigRead)),
    ("GET", "/api/v1/upstreams", Requires(ConfigRead)),
    ("POST", "/api/v1/upstreams", Requires(ConfigWrite)),
    ("GET", "/api/v1/upstreams/{id}", Requires(ConfigRead)),
    ("PUT", "/api/v1/upstreams/{id}", Requires(ConfigWrite)),
    ("DELETE", "/api/v1/upstreams/{id}", Requires(ConfigWrite)),
    (
        "POST",
        "/api/v1/upstreams/{id}/nodes",
        Requires(ConfigWrite),
    ),
    (
        "PUT",
        "/api/v1/upstreams/{id}/nodes/{node}",
        Requires(ConfigWrite),
    ),
    (
        "DELETE",
        "/api/v1/upstreams/{id}/nodes/{node}",
        Requires(ConfigWrite),
    ),
    ("GET", "/api/v1/listeners", Requires(ConfigRead)),
    ("GET", "/api/v1/listeners/{id}", Requires(ConfigRead)),
    ("PUT", "/api/v1/listeners/{id}", Requires(ConfigWrite)),
    ("DELETE", "/api/v1/listeners/{id}", Requires(ConfigWrite)),
    ("GET", "/api/v1/tls-profiles", Requires(ConfigRead)),
    ("GET", "/api/v1/tls-profiles/{id}", Requires(ConfigRead)),
    ("PUT", "/api/v1/tls-profiles/{id}", Requires(ConfigWrite)),
    ("DELETE", "/api/v1/tls-profiles/{id}", Requires(ConfigWrite)),
    ("GET", "/api/v1/security-policies", Requires(ConfigRead)),
    (
        "GET",
        "/api/v1/security-policies/{id}",
        Requires(ConfigRead),
    ),
    (
        "PUT",
        "/api/v1/security-policies/{id}",
        Requires(ConfigWrite),
    ),
    (
        "DELETE",
        "/api/v1/security-policies/{id}",
        Requires(ConfigWrite),
    ),
    ("GET", "/api/v1/config/draft", Requires(ConfigRead)),
    ("GET", "/api/v1/config/validation", Requires(ConfigRead)),
    ("POST", "/api/v1/config/apply", Requires(ConfigApply)),
    ("GET", "/api/v1/approval-policies", Requires(ConfigRead)),
    (
        "GET",
        "/api/v1/approval-policies/{id}",
        Requires(ConfigRead),
    ),
    (
        "PUT",
        "/api/v1/approval-policies/{id}",
        Requires(ApprovalManage),
    ),
    (
        "DELETE",
        "/api/v1/approval-policies/{id}",
        Requires(ApprovalManage),
    ),
    ("GET", "/api/v1/approvals", Requires(ConfigRead)),
    ("GET", "/api/v1/approvals/{id}", Requires(ConfigRead)),
    (
        "POST",
        "/api/v1/approvals/{id}/approve",
        Requires(ApprovalDecide),
    ),
    (
        "POST",
        "/api/v1/approvals/{id}/reject",
        Requires(ApprovalDecide),
    ),
    (
        "POST",
        "/api/v1/approvals/{id}/revoke",
        Requires(ApprovalDecide),
    ),
    (
        "POST",
        "/api/v1/approvals/{id}/withdraw",
        Requires(ConfigApply),
    ),
    ("GET", "/api/v1/config/source", Requires(ConfigRead)),
    ("PUT", "/api/v1/config/source", Requires(ConfigWrite)),
    ("POST", "/api/v1/config/check", Requires(ConfigRead)),
    ("POST", "/api/v1/config/format", Requires(ConfigRead)),
    ("GET", "/api/v1/config/schema", Requires(ConfigRead)),
    ("POST", "/api/v1/config/ast", Requires(ConfigRead)),
    ("POST", "/api/v1/config/explain", Requires(ConfigRead)),
    ("GET", "/api/v1/config/ir", Requires(ConfigRead)),
    ("POST", "/api/v1/config/import/nginx", Requires(ConfigRead)),
    ("GET", "/api/v1/config/plan", Requires(ConfigRead)),
    ("POST", "/api/v1/config/dry-run", Requires(ConfigApply)),
    ("GET", "/api/v1/revisions", Requires(ConfigRead)),
    ("GET", "/api/v1/revisions/{id}", Requires(ConfigRead)),
    ("GET", "/api/v1/revisions/{id}/diff", Requires(ConfigRead)),
    (
        "POST",
        "/api/v1/revisions/{id}/restore",
        Requires(ConfigWrite),
    ),
    ("PUT", "/api/v1/revisions/{id}/note", Requires(ConfigWrite)),
    ("GET", "/api/v1/certificates", Requires(CertificateRead)),
    ("POST", "/api/v1/certificates", Requires(CertificateManage)),
    (
        "GET",
        "/api/v1/certificates/{id}",
        Requires(CertificateRead),
    ),
    (
        "PUT",
        "/api/v1/certificates/{id}",
        Requires(CertificateManage),
    ),
    (
        "DELETE",
        "/api/v1/certificates/{id}",
        Requires(CertificateManage),
    ),
    (
        "GET",
        "/api/v1/certificates/{id}/coverage",
        Requires(CertificateRead),
    ),
    (
        "POST",
        "/api/v1/certificate-inspections",
        Requires(CertificateManage),
    ),
    ("GET", "/api/v1/acme-accounts", Requires(CertificateRead)),
    ("POST", "/api/v1/acme-accounts", Requires(CertificateManage)),
    (
        "GET",
        "/api/v1/acme-accounts/{id}",
        Requires(CertificateRead),
    ),
    (
        "DELETE",
        "/api/v1/acme-accounts/{id}",
        Requires(CertificateManage),
    ),
    (
        "GET",
        "/api/v1/acme-certificates",
        Requires(CertificateRead),
    ),
    (
        "POST",
        "/api/v1/acme-certificates",
        Requires(CertificateManage),
    ),
    (
        "GET",
        "/api/v1/acme-certificates/{id}",
        Requires(CertificateRead),
    ),
    (
        "DELETE",
        "/api/v1/acme-certificates/{id}",
        Requires(CertificateManage),
    ),
    (
        "POST",
        "/api/v1/acme-certificates/{id}/renewals",
        Requires(CertificateManage),
    ),
    ("GET", "/api/v1/dns-providers", Requires(CertificateRead)),
    ("POST", "/api/v1/dns-providers", Requires(CertificateManage)),
    (
        "GET",
        "/api/v1/dns-providers/{id}",
        Requires(CertificateRead),
    ),
    (
        "PUT",
        "/api/v1/dns-providers/{id}",
        Requires(CertificateManage),
    ),
    (
        "DELETE",
        "/api/v1/dns-providers/{id}",
        Requires(CertificateManage),
    ),
    ("POST", "/api/v1/tls-checks", Requires(GatewayRead)),
    ("GET", "/api/v1/traffic", Requires(GatewayRead)),
    ("GET", "/api/v1/traffic/series", Requires(GatewayRead)),
    ("GET", "/api/v1/logs", Requires(LogsRead)),
    ("GET", "/api/v1/logs/download", Requires(LogsRead)),
    ("GET", "/api/v1/logs/tail", Requires(LogsRead)),
    ("CONNECT", "/api/v1/logs/tail", Requires(LogsRead)),
    ("GET", "/api/v1/logs/deletions", Requires(LogsRead)),
    ("POST", "/api/v1/logs/deletions", Requires(LogsDelete)),
    ("GET", "/api/v1/host", Requires(HostRead)),
    ("GET", "/api/v1/host/agent", Requires(HostRead)),
    ("GET", "/api/v1/host/directories", Requires(HostRead)),
    ("GET", "/api/v1/host/listeners", Requires(HostRead)),
    ("GET", "/api/v1/host/gateway-service", Requires(HostRead)),
    (
        "POST",
        "/api/v1/host/gateway-service/{action}",
        Requires(HostManage),
    ),
    ("GET", "/api/v1/container-engines", Requires(ContainersRead)),
    (
        "POST",
        "/api/v1/container-engines/{engine}/enable",
        Requires(ContainersManage),
    ),
    (
        "POST",
        "/api/v1/container-engines/{engine}/disable",
        Requires(ContainersManage),
    ),
    (
        "GET",
        "/api/v1/container-engines/{engine}/containers",
        Requires(ContainersRead),
    ),
    ("GET", "/api/v1/alert-rules", Requires(AlertsRead)),
    ("PUT", "/api/v1/alert-rules/{id}", Requires(AlertsManage)),
    ("DELETE", "/api/v1/alert-rules/{id}", Requires(AlertsManage)),
    ("GET", "/api/v1/alert-channels", Requires(AlertsRead)),
    ("POST", "/api/v1/alert-channels", Requires(AlertsManage)),
    (
        "DELETE",
        "/api/v1/alert-channels/{id}",
        Requires(AlertsManage),
    ),
    (
        "POST",
        "/api/v1/alert-channels/{id}/rotate",
        Requires(AlertsManage),
    ),
    (
        "POST",
        "/api/v1/alert-channels/{id}/test",
        Requires(AlertsManage),
    ),
    ("GET", "/api/v1/alert-notifications", Requires(AlertsRead)),
    ("GET", "/api/v1/audit-events", Requires(AuditRead)),
    ("GET", "/api/v1/audit-events/verify", Requires(AuditRead)),
    (
        "GET",
        "/api/v1/audit-events/{sequence}",
        Requires(AuditRead),
    ),
];

/// A request refused to an authenticated caller.
#[derive(Clone, Debug)]
pub struct Refusal {
    pub method: String,
    /// The route's path template, such as `/api/v1/sites/{id}`.
    pub route: String,
    /// `permission`, `csrf` or `cross_site`.
    pub reason: &'static str,
    /// The permission the route needs, when that was missing.
    pub permission: Option<&'static str>,
}

/// Records refused requests of authenticated callers for the audit trail.
#[async_trait]
pub trait AccessAudit: Send + Sync {
    async fn denied(&self, principal: &Principal, refusal: &Refusal, scope: &RequestScope);
}

pub(crate) fn access(method: &Method, path: &str) -> Option<Access> {
    let method = if method == Method::HEAD {
        "GET"
    } else {
        method.as_str()
    };
    ROUTES
        .iter()
        .find(|(candidate, template, _)| *candidate == method && *template == path)
        .map(|(_, _, access)| *access)
}

/// How the API authenticates and protects its callers.
#[derive(Clone, Debug)]
pub struct AccessSettings {
    /// Origins the panel is reached at, such as `https://panel.example`,
    /// for requests without `Sec-Fetch-Site`; the request's own host when
    /// empty.
    pub origins: Vec<String>,
    /// Login attempts a client address may make in a burst, refilled at
    /// one per `login_refill`.
    pub login_burst: NonZeroU32,
    pub login_refill: Duration,
}

impl Default for AccessSettings {
    fn default() -> Self {
        Self {
            origins: Vec::new(),
            login_burst: NonZeroU32::new(10).expect("non-zero"),
            login_refill: Duration::from_secs(6),
        }
    }
}

/// Identity with the settings the routes need.
pub(crate) struct Gate {
    pub(crate) identity: Identity,
    settings: AccessSettings,
    logins: DefaultKeyedRateLimiter<String>,
}

impl Gate {
    pub(crate) fn new(identity: Identity, settings: AccessSettings) -> Arc<Self> {
        let quota = Quota::with_period(settings.login_refill)
            .unwrap_or_else(|| Quota::per_second(NonZeroU32::MIN))
            .allow_burst(settings.login_burst);
        Arc::new(Self {
            identity,
            settings,
            logins: RateLimiter::keyed(quota),
        })
    }

    /// Admits a login attempt from `client`, or says when one will be.
    pub(crate) fn admit_login(&self, client: &Client) -> Result<(), ApiError> {
        if self.logins.len() > 10_000 {
            self.logins.retain_recent();
        }
        let key = client.address.clone().unwrap_or_default();
        self.logins.check_key(&key).map_err(|not_until| {
            let wait = not_until.wait_time_from(governor::clock::Clock::now(
                &governor::clock::DefaultClock::default(),
            ));
            ApiError::new(PanelError::resource_exhausted(
                "too many login attempts from this address; try again shortly",
            ))
            .with_retry_after(wait)
        })
    }
}

fn header(headers: &HeaderMap, name: impl header::AsHeaderName) -> Option<&str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// The value of cookie `name` among the request's cookies.
pub(crate) fn cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(Cookie::split_parse)
        .filter_map(Result::ok)
        .find(|cookie| cookie.name() == name)
        .map(|cookie| cookie.value().to_owned())
        .filter(|value| !value.is_empty())
}

/// A `Set-Cookie` value for a `__Host-` cookie holding `value` for
/// `max_age`, or clearing it. The prefix requires `Secure` and `Path=/`.
pub(crate) fn host_cookie(
    name: &'static str,
    value: Option<&str>,
    same_site: SameSite,
    max_age: Duration,
) -> HeaderValue {
    let (value, max_age) = value.map_or(("", Duration::ZERO), |value| (value, max_age));
    let cookie = Cookie::build((name, value))
        .path("/")
        .secure(true)
        .http_only(true)
        .same_site(same_site)
        .max_age(cookie::time::Duration::seconds(
            i64::try_from(max_age.as_secs()).unwrap_or(i64::MAX),
        ));
    HeaderValue::from_str(&cookie.to_string()).expect("cookie values are header-safe")
}

/// A `Set-Cookie` value holding the session for `max_age`, or clearing it.
pub(crate) fn session_cookie(secret: Option<&str>, max_age: Duration) -> HeaderValue {
    host_cookie(SESSION_COOKIE, secret, SameSite::Strict, max_age)
}

/// Where a request comes from: the peer, or the last forwarded address
/// when a local proxy forwarded it.
pub(crate) fn client(request_headers: &HeaderMap, peer: Option<SocketAddr>) -> Client {
    let forwarded = peer
        .filter(|peer| peer.ip().is_loopback())
        .and_then(|_| header(request_headers, "x-forwarded-for"))
        .and_then(|value| value.rsplit(',').next())
        .map(str::trim)
        .filter(|value| value.parse::<std::net::IpAddr>().is_ok())
        .map(str::to_owned);
    Client {
        address: forwarded.or_else(|| peer.map(|peer| peer.ip().to_string())),
        user_agent: header(request_headers, header::USER_AGENT)
            .map(|agent| agent.chars().take(256).collect()),
    }
}

/// Refuses unsafe requests a browser sent from another site.
fn same_origin(headers: &HeaderMap, settings: &AccessSettings) -> Result<(), ApiError> {
    let refused = || {
        ApiError::new(PanelError::permission_denied(
            "cross-site requests are refused",
        ))
    };
    match header(headers, "sec-fetch-site") {
        Some("same-origin" | "none") => Ok(()),
        Some(_) => Err(refused()),
        None => match header(headers, header::ORIGIN) {
            None => Ok(()),
            Some(origin) => {
                let allowed = if settings.origins.is_empty() {
                    let host = header(headers, header::HOST).unwrap_or_default();
                    origin
                        .split_once("://")
                        .is_some_and(|(_, authority)| authority == host)
                } else {
                    settings.origins.iter().any(|allowed| allowed == origin)
                };
                if allowed {
                    Ok(())
                } else {
                    Err(refused())
                }
            }
        },
    }
}

async fn authenticate(
    identity: &Identity,
    headers: &HeaderMap,
) -> Result<Option<Principal>, ApiError> {
    if let Some(credential) = header(headers, header::AUTHORIZATION) {
        let Some(secret) = credential
            .strip_prefix("Bearer ")
            .or_else(|| credential.strip_prefix("bearer "))
            .map(str::trim)
        else {
            return Ok(None);
        };
        let principal = if secret.starts_with(TOKEN_PREFIX) {
            identity.authenticate_token(secret).await?
        } else {
            identity
                .authenticate_session(secret, Transport::Bearer)
                .await?
        };
        return Ok(principal);
    }
    match cookie(headers, SESSION_COOKIE) {
        Some(secret) => Ok(identity
            .authenticate_session(&secret, Transport::Cookie)
            .await?),
        None => Ok(None),
    }
}

fn unsafe_method(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// Whether a request opens a WebSocket, over HTTP/1.1 (RFC 6455) or
/// HTTP/2 (RFC 8441).
fn websocket(method: &Method, headers: &HeaderMap) -> bool {
    *method == Method::CONNECT
        || header(headers, header::UPGRADE)
            .is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
}

/// The guard every route passes when the API authenticates its callers.
pub(crate) async fn guard<U: Send + Sync + 'static>(
    State(state): State<ApiState<U>>,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(gate) = state.identity.clone() else {
        return next.run(request).await;
    };
    request.headers_mut().remove(ACTOR_HEADER);
    request.headers_mut().remove(SITE_SCOPE_HEADER);
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(|matched| matched.as_str().to_owned())
        .unwrap_or_default();
    let access = access(request.method(), &route);
    let Some(access) = access else {
        return ApiError::new(PanelError::permission_denied(
            "the route has no access rule",
        ))
        .into_response();
    };
    let unsafe_request = unsafe_method(request.method());
    if access == Public {
        if unsafe_request {
            if let Err(error) = same_origin(request.headers(), &gate.settings) {
                return error.into_response();
            }
        }
        return next.run(request).await;
    }
    let principal = match authenticate(&gate.identity, request.headers()).await {
        Ok(Some(principal)) => principal,
        Ok(None) => {
            let mut response = ApiError::new(PanelError::unauthenticated(
                "log in or present an API token",
            ))
            .into_response();
            response
                .headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
            return response;
        }
        Err(error) => return error.into_response(),
    };
    let method = request.method().to_string();
    let scope = request_scope(request.headers()).ok();
    let refused = |reason: &'static str, permission: Option<Permission>, error: ApiError| {
        let refusal = Refusal {
            method: method.clone(),
            route: route.clone(),
            reason,
            permission: permission.map(Permission::name),
        };
        let scope = scope.clone();
        let audit = state.access_audit.clone();
        let principal = principal.clone();
        async move {
            if let (Some(audit), Some(scope)) = (audit, scope) {
                audit.denied(&principal, &refusal, &scope).await;
            }
            error.into_response()
        }
    };
    // Browsers send cookies with WebSocket handshakes but cannot add the
    // CSRF header to them, so their origin is what keeps other sites out.
    let websocket = websocket(request.method(), request.headers());
    if (unsafe_request || websocket) && principal.csrf_token().is_some() {
        if let Err(error) = same_origin(request.headers(), &gate.settings) {
            return refused("cross_site", None, error).await;
        }
        if !websocket && !principal.csrf_matches(header(request.headers(), CSRF_HEADER)) {
            let error = ApiError::new(PanelError::permission_denied(
                "the request lacks the session's CSRF token",
            ));
            return refused("csrf", None, error).await;
        }
    }
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(address)| *address);
    let held = principal.access(
        Utc::now(),
        client(request.headers(), peer)
            .address
            .and_then(|address| address.parse().ok()),
    );
    if let Requires(permission) = access {
        if !held.unrestricted.contains(permission) {
            if !held.holds(permission) {
                let error = ApiError::new(PanelError::permission_denied(format!(
                    "this needs the {} permission",
                    permission.name()
                )));
                return refused("permission", Some(permission), error).await;
            }
            // Held only for some sites: the configuration service keeps the
            // request within them.
            if let Ok(value) = serde_json::to_string(&site_scope(&held))
                .map_err(|_| ())
                .and_then(|scope| HeaderValue::from_str(&scope).map_err(|_| ()))
            {
                request.headers_mut().insert(SITE_SCOPE_HEADER, value);
            }
        }
    }
    request.extensions_mut().insert(held);
    if let Ok(actor) = HeaderValue::from_str(principal.actor()) {
        request.headers_mut().insert(ACTOR_HEADER, actor);
    }
    request.extensions_mut().insert(principal);
    next.run(request).await
}
