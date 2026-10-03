use super::{runtime::FakeRuntime, FakeGateway, IdentityCompiler};
use crate::{access::ROUTES, router, AccessAudit, AccessSettings, ApiDoc, ApiState, Refusal};
use async_trait::async_trait;
use axum::{
    body::Body,
    http::{header, HeaderMap, Request, StatusCode},
    Router,
};
use panel_application::{GatewayService, RequestScope};
use panel_identity::{
    memory::MemoryIdentityStore, Identity, IdentitySettings, Principal, SecretHash,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    num::NonZeroU32,
    sync::{Arc, Mutex},
    time::Duration,
};
use tower::ServiceExt;
use utoipa::OpenApi;

const BOOTSTRAP: &str = "the-bootstrap-token";
const PASSWORD: &str = "glacier violin tapestry orbit";

#[derive(Default)]
struct RecordedRefusals(Mutex<Vec<(String, Refusal)>>);

#[async_trait]
impl AccessAudit for RecordedRefusals {
    async fn denied(&self, principal: &Principal, refusal: &Refusal, _scope: &RequestScope) {
        self.0
            .lock()
            .unwrap()
            .push((principal.actor().to_owned(), refusal.clone()));
    }
}

fn app_with(settings: AccessSettings, runtime: Arc<FakeRuntime>) -> Router {
    app_recording(settings, runtime, Arc::default())
}

fn app_recording(
    settings: AccessSettings,
    runtime: Arc<FakeRuntime>,
    refusals: Arc<RecordedRefusals>,
) -> Router {
    let identity = Identity::new(
        Arc::new(MemoryIdentityStore::default()),
        IdentitySettings {
            bootstrap: Some(SecretHash::of(BOOTSTRAP)),
            ..IdentitySettings::default()
        },
    );
    router(
        ApiState::new(Arc::new(GatewayService::new(
            Arc::new(FakeGateway),
            Arc::new(IdentityCompiler),
        )))
        .with_runtime(runtime)
        .with_identity(identity, settings)
        .with_access_audit(refusals),
    )
}

fn app() -> Router {
    app_with(AccessSettings::default(), Arc::default())
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: Value,
}

async fn send(app: &Router, request: Request<Body>) -> Reply {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    Reply {
        status,
        headers,
        body: serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    }
}

fn request(method: &str, uri: &str, body: Option<Value>) -> axum::http::request::Builder {
    let builder = Request::builder().method(method).uri(uri);
    match body {
        Some(_) => builder.header(header::CONTENT_TYPE, "application/json"),
        None => builder,
    }
}

fn build(builder: axum::http::request::Builder, body: Option<Value>) -> Request<Body> {
    builder
        .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
        .unwrap()
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    bearer: Option<&str>,
    body: Option<Value>,
) -> Reply {
    let mut builder = request(method, uri, body.clone());
    if let Some(secret) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {secret}"));
    }
    send(app, build(builder, body)).await
}

async fn set_up(app: &Router) {
    let reply = call(
        app,
        "POST",
        "/api/v1/setup",
        None,
        Some(json!({"token": BOOTSTRAP, "username": "root", "password": PASSWORD})),
    )
    .await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.body);
}

async fn bearer(app: &Router, username: &str) -> String {
    let reply = call(
        app,
        "POST",
        "/api/v1/session",
        None,
        Some(json!({"username": username, "password": PASSWORD, "transport": "bearer"})),
    )
    .await;
    assert_eq!(reply.status, StatusCode::CREATED, "{}", reply.body);
    assert!(reply.headers.get(header::SET_COOKIE).is_none());
    reply.body["secret"].as_str().unwrap().to_owned()
}

#[test]
fn every_documented_route_has_exactly_one_access_rule() {
    let document = serde_json::to_value(ApiDoc::openapi()).unwrap();
    let mut documented = BTreeSet::new();
    for (path, item) in document["paths"].as_object().unwrap() {
        for method in ["get", "post", "put", "patch", "delete"] {
            if item.get(method).is_some() {
                documented.insert((method.to_uppercase(), path.clone()));
            }
        }
    }
    let ruled: BTreeSet<_> = ROUTES
        .iter()
        .map(|(method, path, _)| ((*method).to_owned(), (*path).to_owned()))
        .collect();
    assert_eq!(ruled.len(), ROUTES.len(), "a route has two rules");
    assert_eq!(documented, ruled);
}

#[tokio::test]
async fn requests_need_a_credential_and_the_route_permission() {
    let app = app();
    let anonymous = call(&app, "GET", "/api/v1/gateway/status", None, None).await;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    assert_eq!(anonymous.headers[header::WWW_AUTHENTICATE], "Bearer");
    assert_eq!(
        call(&app, "GET", "/api/v1/setup", None, None).await.body,
        json!({"required": true})
    );
    let wrong = call(
        &app,
        "POST",
        "/api/v1/setup",
        None,
        Some(json!({"token": "guess", "username": "root", "password": PASSWORD})),
    )
    .await;
    assert_eq!(wrong.status, StatusCode::FORBIDDEN);
    set_up(&app).await;

    let root = bearer(&app, "root").await;
    let status = call(&app, "GET", "/api/v1/gateway/status", Some(&root), None).await;
    assert_eq!(status.status, StatusCode::OK, "{}", status.body);
    let created = call(
        &app,
        "POST",
        "/api/v1/accounts",
        Some(&root),
        Some(json!({"username": "watcher", "password": PASSWORD, "roles": ["viewer"]})),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    assert_eq!(created.body["roles"], json!(["viewer"]));

    let viewer = bearer(&app, "watcher").await;
    let refused = call(&app, "GET", "/api/v1/accounts", Some(&viewer), None).await;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert!(refused.body["detail"]
        .as_str()
        .unwrap()
        .contains("identity.read"));
    assert_eq!(
        call(&app, "GET", "/api/v1/gateway/status", Some(&viewer), None)
            .await
            .status,
        StatusCode::OK
    );
    let current = call(&app, "GET", "/api/v1/session", Some(&viewer), None).await;
    assert_eq!(current.body["account"]["username"], "watcher");
    assert_eq!(current.body["credential"], "bearer");
    assert!(current.body["csrf_token"].is_null());

    let granted = call(
        &app,
        "POST",
        "/api/v1/account/tokens",
        Some(&viewer),
        Some(json!({"name": "monitor", "permissions": ["gateway.read"], "expires_in_days": 30})),
    )
    .await;
    assert_eq!(granted.status, StatusCode::CREATED, "{}", granted.body);
    let token = granted.body["secret"].as_str().unwrap().to_owned();
    assert!(token.starts_with("ppat_"));
    assert_eq!(
        call(&app, "GET", "/api/v1/gateway/status", Some(&token), None)
            .await
            .status,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "GET", "/api/v1/platform/services", Some(&token), None)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let beyond = call(
        &app,
        "POST",
        "/api/v1/account/tokens",
        Some(&viewer),
        Some(json!({"name": "more", "permissions": ["config.write"], "expires_in_days": 30})),
    )
    .await;
    assert_eq!(beyond.status, StatusCode::FORBIDDEN);
    assert_eq!(
        call(
            &app,
            "GET",
            "/api/v1/gateway/status",
            Some("ppat_forged"),
            None
        )
        .await
        .status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn the_actor_is_the_caller_whatever_the_request_claims() {
    let runtime = Arc::new(FakeRuntime::default());
    let app = app_with(AccessSettings::default(), Arc::clone(&runtime));
    set_up(&app).await;
    let root = bearer(&app, "root").await;
    let reload = send(
        &app,
        build(
            request("POST", "/api/v1/gateway/reload", None)
                .header(header::AUTHORIZATION, format!("Bearer {root}"))
                .header("x-actor", "mallory")
                .header("x-deadline", "2099-01-01T00:00:00Z")
                .header("idempotency-key", "reload-1"),
            None,
        ),
    )
    .await;
    assert_eq!(reload.status, StatusCode::OK, "{}", reload.body);
    assert_eq!(*runtime.calls.lock().unwrap(), ["reload root"]);
}

#[tokio::test]
async fn cookie_sessions_need_their_csrf_token_and_the_same_site() {
    let app = app();
    set_up(&app).await;
    let login = call(
        &app,
        "POST",
        "/api/v1/session",
        None,
        Some(json!({"username": "root", "password": PASSWORD})),
    )
    .await;
    assert_eq!(login.status, StatusCode::CREATED, "{}", login.body);
    let set_cookie = login.headers[header::SET_COOKIE].to_str().unwrap();
    assert!(
        set_cookie.starts_with("__Host-ppanel_session="),
        "{set_cookie}"
    );
    assert!(set_cookie.ends_with("; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=86400"));
    assert!(login.body["secret"].is_null());
    assert_eq!(login.headers[header::CACHE_CONTROL], "no-store");
    let cookie = set_cookie.split(';').next().unwrap().to_owned();
    let csrf = login.body["csrf_token"].as_str().unwrap().to_owned();

    let with_cookie = |method: &str, uri: &str, body: Option<Value>| {
        request(method, uri, body).header(header::COOKIE, format!("theme=dark; {cookie}"))
    };
    let current = send(
        &app,
        build(with_cookie("GET", "/api/v1/session", None), None),
    )
    .await;
    assert_eq!(current.status, StatusCode::OK);
    assert_eq!(current.body["csrf_token"], csrf.as_str());
    assert_eq!(current.body["credential"], "cookie");
    assert_eq!(current.body["session"]["current"], true);

    let token = || Some(json!({"name": "ci", "expires_in_days": 7}));
    let without = send(
        &app,
        build(
            with_cookie("POST", "/api/v1/account/tokens", token()),
            token(),
        ),
    )
    .await;
    assert_eq!(without.status, StatusCode::FORBIDDEN);
    assert!(without.body["detail"].as_str().unwrap().contains("CSRF"));
    let cross_site = send(
        &app,
        build(
            with_cookie("POST", "/api/v1/account/tokens", token())
                .header("x-csrf-token", &csrf)
                .header("sec-fetch-site", "cross-site"),
            token(),
        ),
    )
    .await;
    assert_eq!(cross_site.status, StatusCode::FORBIDDEN);
    let foreign_origin = send(
        &app,
        build(
            with_cookie("POST", "/api/v1/account/tokens", token())
                .header("x-csrf-token", &csrf)
                .header(header::HOST, "panel.example")
                .header(header::ORIGIN, "https://elsewhere.example"),
            token(),
        ),
    )
    .await;
    assert_eq!(foreign_origin.status, StatusCode::FORBIDDEN);
    for (name, value) in [
        ("sec-fetch-site", "same-origin"),
        ("origin", "https://panel.example"),
    ] {
        let granted = send(
            &app,
            build(
                with_cookie("POST", "/api/v1/account/tokens", token())
                    .header("x-csrf-token", &csrf)
                    .header(header::HOST, "panel.example")
                    .header(name, value),
                token(),
            ),
        )
        .await;
        assert_eq!(granted.status, StatusCode::CREATED, "{}", granted.body);
    }

    // Logging in from another site is refused too.
    let body = Some(json!({"username": "root", "password": PASSWORD}));
    let forged_login = send(
        &app,
        build(
            request("POST", "/api/v1/session", body.clone()).header("sec-fetch-site", "cross-site"),
            body,
        ),
    )
    .await;
    assert_eq!(forged_login.status, StatusCode::FORBIDDEN);

    let logout = send(
        &app,
        build(
            with_cookie("DELETE", "/api/v1/session", None)
                .header("x-csrf-token", &csrf)
                .header("sec-fetch-site", "same-origin"),
            None,
        ),
    )
    .await;
    assert_eq!(logout.status, StatusCode::NO_CONTENT);
    assert!(logout.headers[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .ends_with("Max-Age=0"));
    let ended = send(
        &app,
        build(with_cookie("GET", "/api/v1/session", None), None),
    )
    .await;
    assert_eq!(ended.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn logins_from_one_address_are_limited() {
    let app = app_with(
        AccessSettings {
            login_burst: NonZeroU32::new(2).unwrap(),
            login_refill: Duration::from_secs(3600),
            ..AccessSettings::default()
        },
        Arc::default(),
    );
    set_up(&app).await;
    let attempt = || {
        call(
            &app,
            "POST",
            "/api/v1/session",
            None,
            Some(json!({"username": "root", "password": "not the password"})),
        )
    };
    assert_eq!(attempt().await.status, StatusCode::UNAUTHORIZED);
    let limited = attempt().await;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(limited.headers.contains_key(header::RETRY_AFTER));
}

#[tokio::test]
async fn refusals_of_authenticated_callers_are_recorded() {
    let refusals = Arc::new(RecordedRefusals::default());
    let app = app_recording(
        AccessSettings::default(),
        Arc::default(),
        Arc::clone(&refusals),
    );
    set_up(&app).await;
    let root = bearer(&app, "root").await;
    call(
        &app,
        "POST",
        "/api/v1/accounts",
        Some(&root),
        Some(json!({"username": "watcher", "password": PASSWORD, "roles": ["viewer"]})),
    )
    .await;
    let viewer = bearer(&app, "watcher").await;
    assert_eq!(
        call(&app, "GET", "/api/v1/accounts", Some(&viewer), None)
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    // Anonymous requests are refused without a record.
    call(&app, "GET", "/api/v1/accounts", None, None).await;

    let login = call(
        &app,
        "POST",
        "/api/v1/session",
        None,
        Some(json!({"username": "root", "password": PASSWORD})),
    )
    .await;
    let cookie = login.headers[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    send(
        &app,
        build(
            request("DELETE", "/api/v1/session", None).header(header::COOKIE, cookie),
            None,
        ),
    )
    .await;

    let recorded = refusals.0.lock().unwrap().clone();
    assert_eq!(recorded.len(), 2, "{recorded:?}");
    assert_eq!(recorded[0].0, "watcher");
    assert_eq!(
        (
            recorded[0].1.method.as_str(),
            recorded[0].1.route.as_str(),
            recorded[0].1.reason,
            recorded[0].1.permission
        ),
        (
            "GET",
            "/api/v1/accounts",
            "permission",
            Some("identity.read")
        )
    );
    assert_eq!(
        (recorded[1].0.as_str(), recorded[1].1.reason),
        ("root", "csrf")
    );
}

#[tokio::test]
async fn roles_sessions_and_tokens_are_managed_through_the_api() {
    let app = app();
    set_up(&app).await;
    let root = bearer(&app, "root").await;
    let created = call(
        &app,
        "POST",
        "/api/v1/roles",
        Some(&root),
        Some(json!({"id": "deployer", "name": "Deployer", "permissions": ["config.read"]})),
    )
    .await;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    assert_eq!(created.body["built_in"], false);
    let replaced = call(
        &app,
        "PUT",
        "/api/v1/roles/deployer",
        Some(&root),
        Some(json!({"name": "Deployer", "permissions": ["config.read", "config.apply"]})),
    )
    .await;
    assert_eq!(
        replaced.body["permissions"],
        json!(["config.read", "config.apply"])
    );
    let built_in = call(
        &app,
        "PUT",
        "/api/v1/roles/administrator",
        Some(&root),
        Some(json!({"name": "Mine", "permissions": ["config.read"]})),
    )
    .await;
    assert_eq!(built_in.status, StatusCode::FORBIDDEN);
    let unknown = call(
        &app,
        "POST",
        "/api/v1/roles",
        Some(&root),
        Some(json!({"id": "x", "name": "X", "permissions": ["root.everything"]})),
    )
    .await;
    assert_eq!(unknown.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        call(&app, "DELETE", "/api/v1/roles/deployer", Some(&root), None)
            .await
            .status,
        StatusCode::NO_CONTENT
    );

    let other = bearer(&app, "root").await;
    let ended = call(
        &app,
        "DELETE",
        "/api/v1/account/sessions",
        Some(&root),
        None,
    )
    .await;
    assert!(ended.body["ended"].as_u64().unwrap() >= 1, "{}", ended.body);
    assert_eq!(
        call(&app, "GET", "/api/v1/session", Some(&other), None)
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, "GET", "/api/v1/session", Some(&root), None)
            .await
            .status,
        StatusCode::OK
    );

    let granted = call(
        &app,
        "POST",
        "/api/v1/account/tokens",
        Some(&root),
        Some(json!({"name": "ci", "permissions": ["gateway.read"], "expires_in_days": 30})),
    )
    .await;
    let old = granted.body["secret"].as_str().unwrap().to_owned();
    let id = granted.body["token"]["id"].as_str().unwrap().to_owned();
    let rotated = call(
        &app,
        "POST",
        &format!("/api/v1/account/tokens/{id}/rotate"),
        Some(&root),
        None,
    )
    .await;
    assert_eq!(rotated.status, StatusCode::CREATED, "{}", rotated.body);
    assert_eq!(rotated.body["token"]["name"], "ci");
    let new = rotated.body["secret"].as_str().unwrap().to_owned();
    assert_eq!(
        call(&app, "GET", "/api/v1/gateway/status", Some(&old), None)
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, "GET", "/api/v1/gateway/status", Some(&new), None)
            .await
            .status,
        StatusCode::OK
    );

    let current = call(&app, "GET", "/api/v1/session", Some(&root), None).await;
    let account = current.body["account"]["id"].as_str().unwrap().to_owned();
    let everything = call(
        &app,
        "DELETE",
        &format!("/api/v1/accounts/{account}/sessions"),
        Some(&root),
        None,
    )
    .await;
    assert_eq!(everything.body["ended"], 1);
    assert_eq!(
        call(&app, "GET", "/api/v1/session", Some(&root), None)
            .await
            .status,
        StatusCode::UNAUTHORIZED
    );
}
