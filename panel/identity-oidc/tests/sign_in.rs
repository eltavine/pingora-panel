#![forbid(unsafe_code)]

//! Signing in through a provider, from the sign-in page to a session.

use chrono::Utc;
use identity_oidc::{testing::TestProvider, OidcClient};
use panel_context::{RequestId, RequestScope};
use panel_identity::{
    memory::MemoryIdentityStore,
    store::{Cause, NewAccount},
    AccountId, ClaimNames, Client, GroupRole, IdentityStore, ProviderDirectory, ProviderRequest,
    ProviderSignIns, ProviderStore, Rechecked, SecretChange, SessionPolicy, Transport, Username,
};
use panel_secrets::EnvelopeVault;
use serde_json::json;
use std::{collections::HashMap, sync::Arc, time::Duration};

fn scope() -> RequestScope {
    RequestScope::new(RequestId::new("req-1").unwrap())
}

async fn follow(url: &str) -> HashMap<String, String> {
    let response = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(url)
        .send()
        .await
        .unwrap();
    let location = response.headers()["location"].to_str().unwrap().to_owned();
    reqwest::Url::parse(&location)
        .unwrap()
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect()
}

#[tokio::test]
async fn people_get_accounts_roles_and_sessions_from_their_provider() {
    let provider = TestProvider::start("panel", Some("s3cret")).await;
    let store = Arc::new(MemoryIdentityStore::default());
    store
        .create_account(
            NewAccount {
                id: AccountId::generate(),
                username: Username::new("root").unwrap(),
                display_name: None,
                password_hash: None,
                roles: vec!["administrator".into()],
                now: Utc::now(),
                first: true,
            },
            &Cause {
                scope: scope(),
                actor: "setup".into(),
            },
        )
        .await
        .unwrap();
    let vault =
        Arc::new(EnvelopeVault::from_keys(&EnvelopeVault::generate_key().unwrap()).unwrap());
    let client = Arc::new(OidcClient::new(Duration::from_secs(5)).unwrap());
    let directory =
        ProviderDirectory::new(store.clone(), store.clone(), vault.clone(), client.clone());
    let request = ProviderRequest {
        display_name: "Corporate".into(),
        issuer: provider.issuer.clone(),
        client_id: "panel".into(),
        client_secret: SecretChange::Set("s3cret".into()),
        scopes: vec!["profile".into()],
        claims: ClaimNames::default(),
        group_roles: vec![
            GroupRole {
                group: "ops".into(),
                role: "operator".into(),
            },
            GroupRole {
                group: "audit".into(),
                role: "auditor".into(),
            },
        ],
        create_accounts: true,
        enabled: true,
    };
    directory
        .put("corp", request.clone(), &scope(), "root")
        .await
        .unwrap();
    let sign_ins = ProviderSignIns::new(
        directory.clone(),
        store.clone(),
        store.clone(),
        client,
        vault,
        "https://panel.example",
        SessionPolicy::default(),
    );
    assert_eq!(sign_ins.options().await.unwrap()[0].id, "corp");
    let browser = Client::default();
    let sign_in = |return_to: &'static str| {
        let sign_ins = sign_ins.clone();
        let browser = browser.clone();
        async move {
            let started = sign_ins.start("corp", Some(return_to)).await.unwrap();
            let callback = follow(&started.url).await;
            sign_ins
                .finish(
                    "corp",
                    &callback["code"],
                    &callback["state"],
                    &started.state,
                    Transport::Cookie,
                    &browser,
                    &scope(),
                )
                .await
        }
    };

    provider.sign_in_as(
        "u-1",
        json!({"preferred_username": "Alice@corp.example", "name": "Alice", "groups": ["ops"]}),
    );
    let (login, return_to) = sign_in("/sites").await.unwrap();
    assert_eq!(return_to, "/sites");
    assert_eq!(login.account.username.as_str(), "alice");
    assert_eq!(login.account.display_name.as_deref(), Some("Alice"));
    assert_eq!(login.account.roles, ["operator"]);

    // An Administrator grants a role by hand; group changes leave it alone.
    store
        .update_account(
            login.account.id,
            panel_identity::AccountChange {
                roles: Some(vec!["operator".into(), "viewer".into()]),
                ..Default::default()
            },
            Utc::now(),
            &Cause {
                scope: scope(),
                actor: "root".into(),
            },
        )
        .await
        .unwrap();
    provider.sign_in_as(
        "u-1",
        json!({"preferred_username": "alice", "groups": ["audit"]}),
    );
    let (login, _) = sign_in("//evil.example").await.unwrap();
    assert_eq!(login.account.roles, ["auditor", "viewer"]);

    let started = sign_ins.start("corp", None).await.unwrap();
    let callback = follow(&started.url).await;
    assert!(
        sign_ins
            .finish(
                "corp",
                &callback["code"],
                &callback["state"],
                "another-browser",
                Transport::Cookie,
                &browser,
                &scope()
            )
            .await
            .is_err(),
        "a callback must come back to the browser that started it"
    );
    assert!(
        sign_ins
            .finish(
                "corp",
                &callback["code"],
                &callback["state"],
                &started.state,
                Transport::Cookie,
                &browser,
                &scope()
            )
            .await
            .is_ok(),
        "the failed attempt did not use the sign-in up"
    );

    // A name taken by a local account is not linked to a stranger.
    provider.sign_in_as("u-2", json!({"preferred_username": "root"}));
    assert!(sign_in("/").await.is_err());

    let mut closed = request;
    closed.create_accounts = false;
    closed.client_secret = SecretChange::Keep;
    directory
        .put("corp", closed, &scope(), "root")
        .await
        .unwrap();
    provider.sign_in_as("u-3", json!({"preferred_username": "carol"}));
    assert!(sign_in("/").await.is_err(), "no account for strangers");
    provider.sign_in_as("u-1", json!({"preferred_username": "alice", "groups": []}));
    let (login, _) = sign_in("/").await.unwrap();
    assert_eq!(
        login.account.roles,
        ["viewer"],
        "linked people still sign in"
    );

    let link = store.link("corp", "u-1").await.unwrap().unwrap();
    assert!(link.granted_roles.is_empty());
    let events: Vec<String> = store
        .events()
        .into_iter()
        .map(|event| event.event_type)
        .filter(|event| event.starts_with("identity.login") || event == "identity.account.created")
        .collect();
    assert_eq!(
        events
            .iter()
            .filter(|event| *event == "identity.account.created")
            .count(),
        2
    );
    let refusals: Vec<String> = store
        .events()
        .into_iter()
        .filter(|event| event.event_type == "identity.login.failed")
        .map(|event| {
            format!(
                "{} {}",
                event.data["attempt"]["provider"], event.data["reason"]
            )
        })
        .collect();
    assert_eq!(
        refusals,
        [
            r#""corp" "provider_state""#,
            r#""corp" "username_taken""#,
            r#""corp" "unknown_account""#,
        ],
        "{events:?}"
    );

    // The provider is asked about sessions every fifteen minutes; it rotates
    // refresh tokens, so the second recheck works only with the kept ones.
    let at = |minutes| {
        sign_ins
            .clone()
            .with_clock(move || Utc::now() + chrono::Duration::minutes(minutes))
    };
    let rechecked = at(16).recheck().await.unwrap();
    assert_eq!(
        (rechecked.kept, rechecked.ended, rechecked.unanswered),
        (4, 0, 0)
    );
    assert_eq!(at(16).recheck().await.unwrap(), Rechecked::default());
    assert_eq!(at(32).recheck().await.unwrap().kept, 4);
    provider.disable("u-1");
    let rechecked = at(48).recheck().await.unwrap();
    assert_eq!((rechecked.kept, rechecked.ended), (0, 4));
    let grant = store.session(&login.secret.hash()).await.unwrap().unwrap();
    assert!(grant.session.revoked_at.is_some());
    let ended = store
        .events()
        .into_iter()
        .filter(|event| event.event_type == "identity.session.ended")
        .count();
    assert_eq!(ended, 4);
}
