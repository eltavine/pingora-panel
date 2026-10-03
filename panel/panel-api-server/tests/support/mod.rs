//! Signing in to a test server.

use reqwest::{
    header::{HeaderMap, HeaderValue, AUTHORIZATION},
    Client, StatusCode,
};
use serde_json::{json, Value};

/// The bootstrap token test servers start with.
pub const BOOTSTRAP: &str = "test-bootstrap-token";
pub const PASSWORD: &str = "glacier violin tapestry orbit";

/// Creates the first account, `operator`, and returns a client whose
/// requests carry its bearer session.
pub async fn signed_in(base: &str) -> Client {
    let client = Client::new();
    let setup = client
        .post(format!("{base}/api/v1/setup"))
        .json(&json!({"token": BOOTSTRAP, "username": "operator", "password": PASSWORD}))
        .send()
        .await
        .unwrap();
    assert_eq!(
        setup.status(),
        StatusCode::CREATED,
        "{}",
        setup.text().await.unwrap()
    );
    let login: Value = client
        .post(format!("{base}/api/v1/session"))
        .json(&json!({"username": "operator", "password": PASSWORD, "transport": "bearer"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let secret = login["secret"].as_str().expect("a bearer session");
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {secret}")).unwrap(),
    );
    let _ = rustls::crypto::ring::default_provider().install_default();
    Client::builder().default_headers(headers).build().unwrap()
}
