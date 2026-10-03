#![forbid(unsafe_code)]

//! The relying party against a provider that checks what a real one does.

use identity_oidc::{testing::TestProvider, OidcClient};
use panel_identity::{ProviderSettings, Refreshed};
use serde_json::json;
use std::{collections::HashMap, time::Duration};

/// Follows the authorization URL as a browser would, up to the callback.
async fn authorize(url: &str) -> HashMap<String, String> {
    let response = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(url)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 303, "{:?}", response.text().await);
    let location = response.headers()["location"].to_str().unwrap().to_owned();
    reqwest::Url::parse(&location)
        .unwrap()
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect()
}

#[tokio::test]
async fn people_sign_in_with_pkce_and_lose_access_with_the_provider() {
    for secret in [Some("s3cret: with/odd+chars"), None] {
        let provider = TestProvider::start("panel", secret).await;
        provider.sign_in_as(
            "u-1",
            json!({"preferred_username": "alice", "groups": ["ops"]}),
        );
        let client = OidcClient::new(Duration::from_secs(5)).unwrap();
        let settings = ProviderSettings {
            issuer: provider.issuer.clone(),
            client_id: "panel".into(),
            client_secret: secret.map(Into::into),
            scopes: vec!["profile".into(), "email".into()],
            redirect_uri: "https://panel.example/api/v1/auth/oidc/corp/callback".into(),
        };
        let metadata = client.discover(&provider.issuer).await.unwrap();
        assert_eq!(metadata.issuer, provider.issuer);

        let request = client.sign_in_request(&settings).await.unwrap();
        assert!(request.url.contains("code_challenge_method=S256"));
        assert!(request.url.contains("scope=openid+profile+email"));
        let callback = authorize(&request.url).await;
        assert_eq!(callback["state"], request.state);
        let wrong_nonce = client
            .complete(&settings, &callback["code"], &request.verifier, "other")
            .await;
        assert!(wrong_nonce.is_err(), "a token for another sign-in");
        let reused = client
            .complete(
                &settings,
                &callback["code"],
                &request.verifier,
                &request.nonce,
            )
            .await;
        assert!(reused.is_err(), "a code works once");

        let request = client.sign_in_request(&settings).await.unwrap();
        let callback = authorize(&request.url).await;
        let wrong_verifier = client
            .complete(
                &settings,
                &callback["code"],
                "not-the-verifier",
                &request.nonce,
            )
            .await;
        assert!(
            wrong_verifier.is_err(),
            "PKCE binds the code to the verifier"
        );

        let request = client.sign_in_request(&settings).await.unwrap();
        let callback = authorize(&request.url).await;
        let signed_in = client
            .complete(
                &settings,
                &callback["code"],
                &request.verifier,
                &request.nonce,
            )
            .await
            .unwrap();
        assert_eq!(signed_in.subject, "u-1");
        assert_eq!(signed_in.claims["preferred_username"], "alice");
        let refresh = signed_in.refresh_token.unwrap();
        let Refreshed::Valid {
            refresh_token: Some(rotated),
        } = client.refresh(&settings, &refresh).await.unwrap()
        else {
            panic!("the provider still vouches")
        };
        provider.disable("u-1");
        assert_eq!(
            client.refresh(&settings, &rotated).await.unwrap(),
            Refreshed::Refused
        );

        let mut other = settings.clone();
        other.issuer = format!("{}/", provider.issuer);
        assert!(
            client.discover(&other.issuer).await.is_err(),
            "the discovery document must name the configured issuer exactly"
        );
    }
}
