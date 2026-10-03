#![forbid(unsafe_code)]

mod support;

use identity_oidc::testing::TestProvider;
use panel_control_runtime::{
    ProcessSettings, DATABASE_PASSWORD_ENV, DATABASE_URL_ENV, NATS_URL_ENV,
};
use panel_jetstream::testing::{TestBroker, NATS_URL_ENV as TEST_NATS_URL_ENV};
use panel_postgres::testing::TestDatabase;
use panel_secrets::EnvelopeVault;
use panel_service::Environment;
use reqwest::{
    header::{COOKIE, LOCATION, SET_COOKIE},
    redirect::Policy,
    Client, Response, StatusCode, Url,
};
use serde_json::{json, Value};
use std::{collections::HashMap, ffi::OsString, net::SocketAddr, time::Duration};

const ORIGIN: &str = "https://panel.test";

fn environment(values: Vec<(&'static str, String)>) -> Environment<'static> {
    let values: HashMap<&str, OsString> = values
        .into_iter()
        .map(|(key, value)| (key, OsString::from(value)))
        .collect();
    Environment::from_lookup(move |name| values.get(name).cloned())
}

fn free_port() -> SocketAddr {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

fn location(response: &Response) -> String {
    response.headers()[LOCATION].to_str().unwrap().to_owned()
}

/// The `name=value` part of the cookie a response sets.
fn set_cookie(response: &Response, name: &str) -> String {
    response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap())
        .find(|value| value.starts_with(&format!("{name}=")))
        .unwrap_or_else(|| panic!("{name} is set"))
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

/// The panel URL the provider sends the browser back to, on the test server.
fn on_server(base: &str, location: &str) -> String {
    let url = Url::parse(location).unwrap();
    assert_eq!(url.origin().ascii_serialization(), ORIGIN);
    format!("{base}{}?{}", url.path(), url.query().unwrap())
}

#[tokio::test]
async fn people_sign_in_through_an_identity_provider() {
    let (Some(mut database), Some(broker)) =
        (TestDatabase::create().await, TestBroker::create().await)
    else {
        return;
    };
    let secrets = database.bootstrap(&[("identity", "identity")]).await;
    let provider = TestProvider::start("panel", Some("client-secret")).await;
    let http = free_port();
    let mut env = environment(vec![
        (DATABASE_URL_ENV, database.service_url("identity")),
        (DATABASE_PASSWORD_ENV, secrets[0].expose().into()),
        (NATS_URL_ENV, std::env::var(TEST_NATS_URL_ENV).unwrap()),
        (panel_api_server::HTTP_ADDRESS_ENV, http.to_string()),
        (
            panel_api_server::BOOTSTRAP_TOKEN_ENV,
            support::BOOTSTRAP.into(),
        ),
        (panel_api_server::PUBLIC_ORIGINS_ENV, ORIGIN.into()),
        (
            panel_api_server::MASTER_KEYS_ENV,
            EnvelopeVault::generate_key().unwrap(),
        ),
    ]);
    let settings = ProcessSettings::read(&mut env, panel_api_server::default_addresses())
        .unwrap()
        .with_listeners(
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:0".parse().unwrap(),
        );
    let _api = panel_api_server::process(&mut env, settings)
        .unwrap()
        .with_jetstream_settings((*broker.settings).clone())
        .start()
        .await
        .unwrap();
    let base = format!("http://{http}");
    let browser = Client::builder().redirect(Policy::none()).build().unwrap();
    for _ in 0..200 {
        if browser
            .get(format!("{base}/api/v1/setup"))
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let admin = support::signed_in(&base).await;
    let created = admin
        .put(format!("{base}/api/v1/identity-providers/corp"))
        .json(&json!({
            "display_name": "Corporate",
            "issuer": provider.issuer,
            "client_id": "panel",
            "client_secret": "client-secret",
            "group_roles": [{"group": "ops", "role": "operator"}],
            "create_accounts": true,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        created.status(),
        StatusCode::CREATED,
        "{}",
        created.text().await.unwrap()
    );
    let created: Value = created.json().await.unwrap();
    assert_eq!(created["has_client_secret"], true);
    assert!(created.get("client_secret").is_none());

    let limit = || {
        admin
            .put(format!("{base}/api/v1/sign-in-policy"))
            .json(&json!({"password_sign_in": "break_glass_only"}))
            .send()
    };
    assert_eq!(
        limit().await.unwrap().status(),
        StatusCode::PRECONDITION_FAILED,
        "a break-glass account must be able to manage accounts first"
    );
    let accounts: Value = admin
        .get(format!("{base}/api/v1/accounts"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let operator = accounts[0]["id"].as_str().unwrap().to_owned();
    let marked: Value = admin
        .patch(format!("{base}/api/v1/accounts/{operator}"))
        .json(&json!({"break_glass": true}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(marked["break_glass"], true);
    assert_eq!(limit().await.unwrap().status(), StatusCode::OK);
    let created = admin
        .post(format!("{base}/api/v1/accounts"))
        .json(&json!({"username": "viewer", "password": support::PASSWORD, "roles": ["viewer"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let password_login = |username: &'static str| {
        browser
            .post(format!("{base}/api/v1/session"))
            .json(&json!({"username": username, "password": support::PASSWORD, "transport": "bearer"}))
            .send()
    };
    assert_eq!(
        password_login("viewer").await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        password_login("operator").await.unwrap().status(),
        StatusCode::CREATED
    );

    let options: Value = browser
        .get(format!("{base}/api/v1/auth/providers"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        options,
        json!([{"id": "corp", "display_name": "Corporate"}])
    );

    provider.sign_in_as(
        "u-1",
        json!({"preferred_username": "alice", "groups": ["ops"]}),
    );
    let start = browser
        .get(format!(
            "{base}/api/v1/auth/oidc/corp/start?return_to=/sites"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(start.status(), StatusCode::SEE_OTHER);
    let state_cookie = set_cookie(&start, "__Host-ppanel_sign_in");
    let authorized = browser.get(location(&start)).send().await.unwrap();
    assert_eq!(authorized.status(), StatusCode::SEE_OTHER);
    let callback = on_server(&base, &location(&authorized));

    let stranger = browser.get(&callback).send().await.unwrap();
    assert_eq!(stranger.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        location(&stranger),
        "/login?sign_in_error=permission_denied",
        "another browser cannot finish the sign-in"
    );

    let start = browser
        .get(format!(
            "{base}/api/v1/auth/oidc/corp/start?return_to=/sites"
        ))
        .send()
        .await
        .unwrap();
    let state_cookie_2 = set_cookie(&start, "__Host-ppanel_sign_in");
    assert_ne!(state_cookie, state_cookie_2);
    let authorized = browser.get(location(&start)).send().await.unwrap();
    let callback = on_server(&base, &location(&authorized));
    let finished = browser
        .get(&callback)
        .header(COOKIE, &state_cookie_2)
        .send()
        .await
        .unwrap();
    assert_eq!(finished.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&finished), "/sites");
    assert_eq!(
        set_cookie(&finished, "__Host-ppanel_sign_in"),
        "__Host-ppanel_sign_in="
    );
    let session = set_cookie(&finished, "__Host-ppanel_session");
    let current: Value = browser
        .get(format!("{base}/api/v1/session"))
        .header(COOKIE, &session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(current["account"]["username"], "alice");
    assert_eq!(current["account"]["roles"], json!(["operator"]));

    let replayed = browser
        .get(&callback)
        .header(COOKIE, &state_cookie_2)
        .send()
        .await
        .unwrap();
    assert!(
        location(&replayed).starts_with("/login?sign_in_error="),
        "a sign-in finishes once"
    );

    let deleted = admin
        .delete(format!("{base}/api/v1/identity-providers/corp"))
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    let ended = browser
        .get(format!("{base}/api/v1/session"))
        .header(COOKIE, &session)
        .send()
        .await
        .unwrap();
    assert_eq!(ended.status(), StatusCode::UNAUTHORIZED);
}
