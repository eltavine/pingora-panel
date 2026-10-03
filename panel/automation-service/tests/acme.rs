#![forbid(unsafe_code)]

//! Automatic certificates against Pebble: accounts, issuance through the
//! gateway's challenge directory and the job worker, renewal windows,
//! failures and their events.

use automation_service::{
    handlers, AccountId, AcmeAutomation, Cause, CertificateInventory, IssuanceState, NewAccount,
    NewAutomaticCertificate, PgJobStore, SecretDirectory, MIGRATIONS,
};
use chrono::Utc;
use panel_acme::{testing::Pebble, AcmeClient, ChallengeKind};
use panel_certificates::{CertificateId, CertificateSource, ACME_CHALLENGE_DIRECTORY};
use panel_errors::ErrorCode;
use panel_events::{Principal, RequestId, RequestScope};
use panel_jobs::{JobStore, Worker, WorkerOptions};
use panel_platform::ServiceName;
use panel_postgres::{testing::TestDatabase, EventLog, ServiceDatabase};
use panel_secrets::{EnvelopeVault, SecretVault};
use std::{
    sync::{Arc, LazyLock},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

static ALICE: LazyLock<Principal> = LazyLock::new(|| EventLog::user("alice"));

fn id(value: &str) -> CertificateId {
    CertificateId::new(value).unwrap()
}

fn cause(scope: &RequestScope) -> Cause<'_> {
    Cause {
        scope,
        principal: &ALICE,
    }
}

async fn database() -> Option<(TestDatabase, ServiceDatabase)> {
    let mut database = TestDatabase::create().await?;
    let secrets = database.bootstrap(&[("automation", "automation")]).await;
    let service = database
        .connect_service("automation", "automation", &secrets[0])
        .await;
    service.migrate(MIGRATIONS).await.unwrap();
    Some((database, service))
}

async fn events(service: &ServiceDatabase) -> Vec<String> {
    let types: Vec<String> = sqlx::query_scalar("SELECT event_type FROM outbox ORDER BY position")
        .fetch_all(service.pool())
        .await
        .unwrap();
    types
        .into_iter()
        .map(|kind| {
            kind.trim_start_matches("io.github.eltavine.pingora-panel.")
                .trim_end_matches(".v1")
                .to_owned()
        })
        .collect()
}

fn automatic(certificate: &str, names: &[&str]) -> NewAutomaticCertificate {
    NewAutomaticCertificate {
        id: id(certificate),
        account: AccountId::new("pebble").unwrap(),
        names: names.iter().map(|name| (*name).to_owned()).collect(),
        challenge: ChallengeKind::Http01,
    }
}

#[tokio::test]
async fn automatic_certificates_are_issued_renewed_and_their_failures_kept() {
    let Some(pebble) = Pebble::from_env() else {
        return;
    };
    let Some((_database, service)) = database().await else {
        return;
    };
    let secrets = tempfile::tempdir().unwrap();
    let vault: Arc<dyn SecretVault> =
        Arc::new(EnvelopeVault::from_keys(&EnvelopeVault::generate_key().unwrap()).unwrap());
    let events_log = EventLog::new(&service, ServiceName::new("automation-service").unwrap());
    let inventory = CertificateInventory::new(
        &service,
        events_log.clone(),
        Some(Arc::clone(&vault)),
        Some(SecretDirectory::new(secrets.path())),
    );
    let jobs = Arc::new(PgJobStore::new(
        &service,
        ServiceName::new("automation-service").unwrap(),
    ));
    let acme = AcmeAutomation::new(
        &service,
        events_log,
        Some(vault),
        inventory.clone(),
        Arc::clone(&jobs) as Arc<dyn JobStore>,
        AcmeClient::new(Duration::from_secs(60)),
        Some(secrets.path()),
    );
    let gateway = pebble
        .serve_http01(&secrets.path().join(ACME_CHALLENGE_DIRECTORY))
        .await;
    let scope = RequestScope::new(RequestId::new("request-1").unwrap());

    let account = acme
        .create_account(
            cause(&scope),
            NewAccount {
                id: AccountId::new("pebble").unwrap(),
                directory: pebble.directory.url.clone(),
                ca_bundle: pebble.directory.ca_bundle.clone(),
                contact: vec!["ops@shop.test".into()],
                terms_of_service_agreed: true,
                external_account: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(account.contact, ["ops@shop.test"]);
    assert!(account.url.starts_with("https://"));
    assert_eq!(
        acme.accounts().await.unwrap(),
        std::slice::from_ref(&account)
    );

    let created = acme
        .create_certificate(
            cause(&scope),
            automatic("shop.test", &["shop.test", "www.shop.test"]),
        )
        .await
        .unwrap();
    assert_eq!(created.state, IssuanceState::Pending);
    let enqueued: i64 =
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE kind = 'certificate.issue'")
            .fetch_one(service.pool())
            .await
            .unwrap();
    assert_eq!(enqueued, 1);
    let wildcard = acme
        .create_certificate(cause(&scope), automatic("wild.test", &["*.wild.test"]))
        .await
        .unwrap_err();
    assert_eq!(wildcard.code.as_str(), ErrorCode::VALIDATION_FAILED);

    // The worker runs the enqueued issuance.
    let stop = CancellationToken::new();
    let worker = handlers(&acme).unwrap().into_iter().fold(
        Worker::new(
            Arc::clone(&jobs) as Arc<dyn JobStore>,
            WorkerOptions::new("test-worker"),
        ),
        |worker, (kind, handler)| worker.with_handler(kind, handler),
    );
    let running = tokio::spawn(worker.run(stop.clone()));
    let issued = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let current = acme.certificate(&id("shop.test")).await.unwrap();
            if current.state != IssuanceState::Pending {
                return current;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await
    .expect("the job issues the certificate");
    stop.cancel();
    running.await.unwrap();
    assert_eq!(issued.state, IssuanceState::Issued, "{issued:?}");
    let certificate = inventory.get(&id("shop.test")).await.unwrap();
    assert_eq!(certificate.source, CertificateSource::Acme);
    assert_eq!(certificate.details.names, ["shop.test", "www.shop.test"]);
    assert_eq!(
        std::fs::read_to_string(secrets.path().join("cert-shop.test.pem")).unwrap(),
        certificate.chain
    );
    assert!(issued.renew_after > Utc::now());
    assert!(issued.renew_after < certificate.details.not_after);

    // Nothing is due, so issuing again changes nothing.
    acme.issue(&scope, &id("shop.test")).await.unwrap();
    assert_eq!(inventory.get(&id("shop.test")).await.unwrap().version, 1);

    // The renewal check follows Pebble's renewal window.
    acme.check_renewals(&scope, Utc::now()).await.unwrap();
    let windowed = acme.certificate(&id("shop.test")).await.unwrap();
    assert!(windowed.renew_after <= certificate.details.not_after);

    acme.renew(cause(&scope), id("shop.test")).await.unwrap();
    acme.issue(&scope, &id("shop.test")).await.unwrap();
    let renewed = inventory.get(&id("shop.test")).await.unwrap();
    assert_eq!(renewed.version, 2);
    assert_ne!(renewed.details.fingerprint, certificate.details.fingerprint);

    // Without a gateway answering, validation fails and is kept.
    drop(gateway);
    acme.create_certificate(cause(&scope), automatic("dark.test", &["dark.test"]))
        .await
        .unwrap();
    let failed = acme.issue(&scope, &id("dark.test")).await.unwrap_err();
    let failing = acme.certificate(&id("dark.test")).await.unwrap();
    assert_eq!(failing.state, IssuanceState::Failing);
    assert_eq!(failing.failures, 1);
    let last = failing.last_error.clone().unwrap();
    assert_eq!(last.code, failed.code.as_str());
    assert!(last.message.starts_with("the CA refused"), "{last:?}");
    let pause = failing.renew_after - Utc::now();
    assert!(pause > chrono::Duration::minutes(55) && pause <= chrono::Duration::hours(1));
    assert!(inventory.get(&id("dark.test")).await.is_err());

    let in_use = acme
        .delete_account(cause(&scope), AccountId::new("pebble").unwrap(), None)
        .await
        .unwrap_err();
    assert_eq!(in_use.code.as_str(), ErrorCode::CONFLICT);
    for certificate in ["dark.test", "shop.test"] {
        acme.delete_certificate(cause(&scope), id(certificate), None)
            .await
            .unwrap();
    }
    assert!(
        inventory.get(&id("shop.test")).await.is_ok(),
        "the certificate stays"
    );
    acme.delete_account(cause(&scope), AccountId::new("pebble").unwrap(), None)
        .await
        .unwrap();

    let types = events(&service).await;
    for expected in [
        "tls.acme.account.created",
        "tls.acme.certificate.created",
        "tls.acme.certificate.refused",
        "tls.certificate.created",
        "tls.acme.certificate.renewal_requested",
        "tls.certificate.replaced",
        "tls.acme.certificate.failed",
        "tls.acme.account.refused",
        "tls.acme.certificate.deleted",
        "tls.acme.account.deleted",
    ] {
        assert!(
            types.iter().any(|kind| kind == expected),
            "{expected} in {types:?}"
        );
    }
}
