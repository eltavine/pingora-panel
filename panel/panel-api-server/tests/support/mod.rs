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

/// Builds the API on a free loopback port of its own. A port found by binding
/// port 0 can be taken by a test running alongside before the API binds it,
/// so another is tried then.
pub fn api_process<P, E: std::fmt::Display>(
    mut build: impl FnMut(std::net::SocketAddr) -> Result<P, E>,
) -> (std::net::SocketAddr, P) {
    for _ in 0..16 {
        let address = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .unwrap();
        match build(address) {
            Ok(process) => return (address, process),
            Err(error) if error.to_string().contains("Address already in use") => continue,
            Err(error) => panic!("{error}"),
        }
    }
    panic!("no loopback port stayed free long enough to bind");
}
