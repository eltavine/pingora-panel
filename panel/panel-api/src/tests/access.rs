use super::{runtime::FakeRuntime, FakeGateway, IdentityCompiler};
use crate::{access::ROUTES, router, AccessSettings, ApiDoc, ApiState};
use axum::{
    body::Body,
    http::{header, HeaderMap, Request, StatusCode},
    Router,
};
use panel_application::GatewayService;
use panel_identity::{memory::MemoryIdentityStore, Identity, IdentitySettings, SecretHash};
use serde_json::{json, Value};
use std::{collections::BTreeSet, num::NonZeroU32, sync::Arc, time::Duration};
use tower::ServiceExt;
use utoipa::OpenApi;

const BOOTSTRAP: &str = "the-bootstrap-token";
const PASSWORD: &str = "glacier violin tapestry orbit";

fn app_with(settings: AccessSettings, runtime: Arc<FakeRuntime>) -> Router {
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
        .with_identity(identity, settings),
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
