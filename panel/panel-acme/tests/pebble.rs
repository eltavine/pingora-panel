#![forbid(unsafe_code)]

//! Accounts, orders and renewal information against Pebble.

use chrono::Utc;
use panel_acme::{
    testing::Pebble, AcmeClient, ChallengeKind, Dns01, DnsProvider, Http01, OrderRequest,
    Registration,
};
use panel_certificates::{accept, renewal_identifier};
use std::{sync::Arc, time::Duration};

fn registration() -> Registration {
    Registration {
        contact: vec!["mailto:ops@shop.test".into()],
        terms_of_service_agreed: true,
        external_account: None,
    }
}

#[tokio::test]
async fn http01_orders_issue_and_renew_with_renewal_information() {
    let Some(pebble) = Pebble::from_env() else {
        return;
    };
    let client = AcmeClient::new(Duration::from_secs(60));
    let account = client
        .register(&pebble.directory, &registration())
        .await
        .unwrap();
    assert!(account.url.starts_with("https://"));
    assert!(!format!("{account:?}").contains("key"));

    let challenges = tempfile::tempdir().unwrap();
    let _server = pebble.serve_http01(challenges.path()).await;
    let solver = Http01::new(challenges.path());
    let names = vec!["shop.test".to_owned(), "www.shop.test".to_owned()];
    let request = OrderRequest {
        names: &names,
        challenge: ChallengeKind::Http01,
        replaces: None,
    };
    let issued = client
        .issue(&pebble.directory, &account.credentials, request, &solver)
        .await
        .unwrap();
    let accepted = accept(&issued.chain, &issued.key, Utc::now()).unwrap();
    assert_eq!(accepted.details.names, names);
    assert!(accepted.details.chain_length >= 2);
    assert!(
        std::fs::read_dir(challenges.path())
            .unwrap()
            .next()
            .is_none(),
        "answered challenges are removed"
    );

    let identifier = renewal_identifier(&issued.chain)
        .unwrap()
        .expect("Pebble names its key");
    let window = client
        .renewal_window(&pebble.directory, &account.credentials, &identifier)
        .await
        .unwrap()
        .expect("Pebble offers renewal information");
    assert!(window.start < window.end);
    assert!(window.end <= accepted.details.not_after);
    assert!(window.next_check > Utc::now());

    let renewed = client
        .issue(
            &pebble.directory,
            &account.credentials,
            OrderRequest {
                replaces: Some(&identifier),
                ..request
            },
            &solver,
        )
        .await
        .unwrap();
    let renewed = accept(&renewed.chain, &renewed.key, Utc::now()).unwrap();
    assert_ne!(renewed.details.fingerprint, accepted.details.fingerprint);
    assert_ne!(
        renewed.details.public_key_fingerprint, accepted.details.public_key_fingerprint,
        "every certificate gets a new key"
    );

    let unanswered = tempfile::tempdir().unwrap();
    let refused = client
        .issue(
            &pebble.directory,
            &account.credentials,
            OrderRequest {
                names: &["elsewhere.test".to_owned()],
                ..request
            },
            &Http01::new(unanswered.path()),
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code.as_str(), "VALIDATION_FAILED", "{refused}");
    assert!(refused.message.starts_with("the CA refused"), "{refused}");

    let wildcard = client
        .issue(
            &pebble.directory,
            &account.credentials,
            OrderRequest {
                names: &["*.shop.test".to_owned()],
                ..request
            },
            &solver,
        )
        .await
        .unwrap_err();
    assert!(wildcard.message.contains("DNS-01"), "{wildcard}");
}

#[tokio::test]
async fn dns01_orders_cover_wildcards() {
    let Some(pebble) = Pebble::from_env() else {
        return;
    };
    let client = AcmeClient::new(Duration::from_secs(60));
    let account = client
        .register(&pebble.directory, &registration())
        .await
        .unwrap();
    let solver = Dns01::new(
        Arc::new(pebble.dns()) as Arc<dyn DnsProvider>,
        Duration::ZERO,
    );
    let names = vec!["wild.test".to_owned(), "*.wild.test".to_owned()];
    let issued = client
        .issue(
            &pebble.directory,
            &account.credentials,
            OrderRequest {
                names: &names,
                challenge: ChallengeKind::Dns01,
                replaces: None,
            },
            &solver,
        )
        .await
        .unwrap();
    let accepted = accept(&issued.chain, &issued.key, Utc::now()).unwrap();
    assert_eq!(accepted.details.names, names);
}

#[tokio::test]
async fn registration_needs_the_terms_agreed_to() {
    let Some(pebble) = Pebble::from_env() else {
        return;
    };
    let error = AcmeClient::default()
        .register(
            &pebble.directory,
            &Registration {
                terms_of_service_agreed: false,
                ..registration()
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code.as_str(), "VALIDATION_FAILED");
}
