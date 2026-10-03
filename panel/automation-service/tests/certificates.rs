#![forbid(unsafe_code)]

use automation_service::{
    AcmeAutomation, Cause, CertificateInventory, CertificateService, SecretDirectory, MIGRATIONS,
};
use chrono::Utc;
use panel_acme::AcmeClient;
use panel_certificates::{self_signed, Accepted, Certificate, CertificateId, CertificateSource};
use panel_contracts::{
    automation::v1::{self as wire, certificates_server::Certificates},
    common::v1 as common,
};
use panel_errors::ErrorCode;
use panel_events::{Principal, RequestId, RequestScope};
use panel_jobs::MemoryJobStore;
use panel_platform::ServiceName;
use panel_postgres::{testing::TestDatabase, EventLog, ServiceDatabase};
use panel_secrets::{EnvelopeVault, SecretVault};
use serde_json::json;
use std::{
    fs,
    path::Path,
    sync::{Arc, LazyLock},
};
use tonic::Request;

fn id(value: &str) -> CertificateId {
    CertificateId::new(value).unwrap()
}

fn material(name: &str) -> Accepted {
    self_signed(&[name.to_owned()], 30, Utc::now()).unwrap()
}

fn vault(keys: &[&str]) -> Arc<dyn SecretVault> {
    Arc::new(EnvelopeVault::from_keys(&keys.join("\n")).unwrap())
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

fn inventory(
    service: &ServiceDatabase,
    vault: Option<Arc<dyn SecretVault>>,
    directory: Option<&Path>,
) -> CertificateInventory {
    CertificateInventory::new(
        service,
        EventLog::new(service, ServiceName::new("automation-service").unwrap()),
        vault,
        directory.map(SecretDirectory::new),
    )
}

fn automation(
    service: &ServiceDatabase,
    vault: Arc<dyn SecretVault>,
    inventory: CertificateInventory,
) -> AcmeAutomation {
    AcmeAutomation::new(
        service,
        EventLog::new(service, ServiceName::new("automation-service").unwrap()),
        Some(vault),
        inventory,
        Arc::new(MemoryJobStore::new()),
        AcmeClient::default(),
        None,
    )
}

async fn events(service: &ServiceDatabase) -> Vec<String> {
    let types: Vec<String> = sqlx::query_scalar("SELECT event_type FROM outbox ORDER BY position")
        .fetch_all(service.pool())
        .await
        .unwrap();
    types
        .into_iter()
        .map(|kind| {
            kind.trim_start_matches("io.github.eltavine.pingora-panel.tls.certificate.")
                .trim_end_matches(".v1")
                .to_owned()
        })
        .collect()
}

async fn sealed_key(service: &ServiceDatabase, id: &str) -> String {
    sqlx::query_scalar("SELECT sealed_key FROM certificates WHERE certificate_id = $1")
        .bind(id)
        .fetch_one(service.pool())
        .await
        .unwrap()
}

fn scope() -> RequestScope {
    RequestScope::new(RequestId::new("request-1").unwrap())
}

static ALICE: LazyLock<Principal> = LazyLock::new(|| EventLog::user("alice"));

fn cause(scope: &RequestScope) -> Cause<'_> {
    Cause {
        scope,
        principal: &ALICE,
    }
}

#[tokio::test]
async fn certificates_are_sealed_delivered_and_published() {
    let Some((_database, service)) = database().await else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let inventory = inventory(
        &service,
        Some(vault(&[&EnvelopeVault::generate_key().unwrap()])),
        Some(directory.path()),
    );
    let scope = scope();
    let first = material("example.com");

    let uploaded = inventory
        .upload(cause(&scope), id("example.com"), &first.chain, &first.key)
        .await
        .unwrap();
    assert_eq!(uploaded.source, CertificateSource::Uploaded);
    assert_eq!(uploaded.version, 1);
    assert_eq!(uploaded.details, first.details);
    assert_eq!(
        inventory.list().await.unwrap(),
        std::slice::from_ref(&uploaded)
    );
    assert_eq!(inventory.get(&id("example.com")).await.unwrap(), uploaded);
    let sealed = sealed_key(&service, "example.com").await;
    assert!(sealed.starts_with("v1.") && !sealed.contains("PRIVATE KEY"));

    let chain_file = directory.path().join("cert-example.com.pem");
    let key_file = directory.path().join("cert-example.com.key");
    assert_eq!(fs::read_to_string(&chain_file).unwrap(), first.chain);
    assert_eq!(fs::read_to_string(&key_file).unwrap(), *first.key);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&key_file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    let again = inventory
        .upload(cause(&scope), id("example.com"), &first.chain, &first.key)
        .await
        .unwrap();
    assert_eq!(again, uploaded);
    let second = material("example.com");
    let taken = inventory
        .upload(cause(&scope), id("example.com"), &second.chain, &second.key)
        .await
        .unwrap_err();
    assert_eq!(taken.code.as_str(), ErrorCode::CONFLICT);
    let invalid = inventory
        .upload(cause(&scope), id("broken"), &first.chain, &second.key)
        .await
        .unwrap_err();
    assert_eq!(invalid.code.as_str(), ErrorCode::VALIDATION_FAILED);

    let stale = inventory
        .replace(
            cause(&scope),
            id("example.com"),
            Some(5),
            &second.chain,
            &second.key,
        )
        .await
        .unwrap_err();
    assert_eq!(stale.code.as_str(), ErrorCode::PRECONDITION_FAILED);
    let replaced = inventory
        .replace(
            cause(&scope),
            id("example.com"),
            Some(1),
            &second.chain,
            &second.key,
        )
        .await
        .unwrap();
    assert_eq!(replaced.version, 2);
    assert_eq!(replaced.created_at, uploaded.created_at);
    assert_eq!(fs::read_to_string(&key_file).unwrap(), *second.key);

    let generated = inventory
        .generate(
            cause(&scope),
            id("internal"),
            &["intranet.example".into()],
            90,
        )
        .await
        .unwrap();
    assert_eq!(generated.source, CertificateSource::SelfSigned);
    assert!(generated.details.self_signed);

    let outdated = inventory
        .delete(cause(&scope), id("example.com"), Some(1))
        .await
        .unwrap_err();
    assert_eq!(outdated.code.as_str(), ErrorCode::PRECONDITION_FAILED);
    inventory
        .delete(cause(&scope), id("example.com"), Some(2))
        .await
        .unwrap();
    assert!(!chain_file.exists() && !key_file.exists());
    let gone = inventory.get(&id("example.com")).await.unwrap_err();
    assert_eq!(gone.code.as_str(), ErrorCode::NOT_FOUND);

    assert_eq!(
        events(&service).await,
        ["created", "refused", "refused", "refused", "replaced", "created", "refused", "deleted"]
    );
}

#[tokio::test]
async fn the_secret_directory_follows_the_inventory() {
    let Some((_database, service)) = database().await else {
        return;
    };
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path();
    let inventory = inventory(
        &service,
        Some(vault(&[&EnvelopeVault::generate_key().unwrap()])),
        Some(path),
    );
    let scope = scope();
    for name in ["a.example", "b.example"] {
        let material = material(name);
        inventory
            .upload(cause(&scope), id(name), &material.chain, &material.key)
            .await
            .unwrap();
    }
    let a = fs::read_to_string(path.join("cert-a.example.pem")).unwrap();
    fs::write(path.join("cert-a.example.pem"), "tampered").unwrap();
    fs::remove_file(path.join("cert-b.example.key")).unwrap();
    fs::write(path.join("cert-ghost.pem"), "left over").unwrap();
    fs::write(path.join("operator.pem"), "placed by hand").unwrap();

    assert_eq!(inventory.reconcile().await.unwrap(), 5);
    assert_eq!(
        fs::read_to_string(path.join("cert-a.example.pem")).unwrap(),
        a
    );
    assert!(path.join("cert-b.example.key").exists());
    assert!(!path.join("cert-ghost.pem").exists());
    assert!(path.join("operator.pem").exists());
    assert_eq!(inventory.reconcile().await.unwrap(), 0);
}

#[tokio::test]
async fn keys_are_sealed_again_when_master_keys_rotate() {
    let Some((_database, service)) = database().await else {
        return;
    };
    let old = EnvelopeVault::generate_key().unwrap();
    let new = EnvelopeVault::generate_key().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let scope = scope();
    let material = material("example.com");
    inventory(&service, Some(vault(&[&old])), None)
        .upload(
            cause(&scope),
            id("example.com"),
            &material.chain,
            &material.key,
        )
        .await
        .unwrap();

    let rotating = inventory(&service, Some(vault(&[&new, &old])), Some(directory.path()));
    assert_eq!(rotating.reseal().await.unwrap(), 1);
    assert_eq!(rotating.reseal().await.unwrap(), 0);

    let retired = inventory(&service, Some(vault(&[&new])), Some(directory.path()));
    assert_eq!(retired.reconcile().await.unwrap(), 2);
    assert_eq!(
        fs::read_to_string(directory.path().join("cert-example.com.key")).unwrap(),
        *material.key
    );
    let forgotten = inventory(&service, Some(vault(&[&old])), Some(directory.path()))
        .reconcile()
        .await
        .unwrap_err();
    assert_eq!(forgotten.code.as_str(), ErrorCode::PRECONDITION_FAILED);
}

#[tokio::test]
async fn without_master_keys_certificates_are_not_stored() {
    let Some((_database, service)) = database().await else {
        return;
    };
    let inventory = inventory(&service, None, None);
    let material = material("example.com");
    let scope = scope();
    let error = inventory
        .upload(
            cause(&scope),
            id("example.com"),
            &material.chain,
            &material.key,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code.as_str(), ErrorCode::UNAVAILABLE);
    assert!(inventory.list().await.unwrap().is_empty());
}

fn context() -> Option<common::RequestContext> {
    Some(common::RequestContext {
        request_id: "request-2".into(),
        actor: "alice".into(),
        ..common::RequestContext::default()
    })
}

async fn change(
    service: &CertificateService,
    operation: &str,
    resource: &str,
    if_match: &str,
    content: serde_json::Value,
) -> wire::ChangeResponse {
    service
        .change(Request::new(wire::ChangeRequest {
            context: context(),
            operation: operation.into(),
            resource: resource.into(),
            if_match: if_match.into(),
            content: serde_json::to_vec(&content).unwrap(),
        }))
        .await
        .unwrap()
        .into_inner()
}

async fn read(service: &CertificateService, operation: &str, resource: &str) -> wire::ReadResponse {
    service
        .read(Request::new(wire::ReadRequest {
            context: context(),
            operation: operation.into(),
            resource: resource.into(),
            parameters: Vec::new(),
        }))
        .await
        .unwrap()
        .into_inner()
}

#[tokio::test]
async fn operations_map_to_the_inventory_over_grpc() {
    let Some((_database, database)) = database().await else {
        return;
    };
    let vault = vault(&[&EnvelopeVault::generate_key().unwrap()]);
    let inventory = inventory(&database, Some(Arc::clone(&vault)), None);
    let acme = automation(&database, vault, inventory.clone());
    let service = CertificateService::new(inventory, acme);
    let first = material("example.com");

    let uploaded = change(
        &service,
        "certificates.upload",
        "certificates",
        "",
        json!({ "id": "example.com", "chain": first.chain, "key": *first.key }),
    )
    .await;
    assert_eq!(uploaded.error, None);
    assert_eq!(uploaded.etag, "\"1\"");
    let certificate: Certificate = serde_json::from_slice(&uploaded.content).unwrap();
    assert_eq!(certificate.details.names, ["example.com"]);
    assert!(!String::from_utf8_lossy(&uploaded.content).contains("PRIVATE KEY"));

    let generated = change(
        &service,
        "certificates.generate",
        "certificates",
        "",
        json!({ "id": "internal", "names": ["intranet.example"], "days": 30 }),
    )
    .await;
    assert_eq!(generated.error, None);

    let listed = read(&service, "certificates.list", "certificates").await;
    let listed: Vec<Certificate> = serde_json::from_slice(&listed.content).unwrap();
    assert_eq!(listed.len(), 2);
    let one = read(&service, "certificates.get", "certificates/example.com").await;
    assert_eq!(one.etag, "\"1\"");

    let second = material("example.com");
    let replaced = change(
        &service,
        "certificates.replace",
        "certificates/example.com",
        "\"1\"",
        json!({ "chain": second.chain, "key": *second.key }),
    )
    .await;
    assert_eq!(replaced.etag, "\"2\"");
    let deleted = change(
        &service,
        "certificates.delete",
        "certificates/internal",
        "",
        json!(null),
    )
    .await;
    assert_eq!(deleted.error, None);

    for (operation, resource, code) in [
        (
            "certificates.get",
            "certificates/missing",
            ErrorCode::NOT_FOUND,
        ),
        (
            "certificates.get",
            "certificates/../etc",
            ErrorCode::INVALID_ARGUMENT,
        ),
        (
            "certificates.list",
            "certificates/example.com",
            ErrorCode::INVALID_ARGUMENT,
        ),
        (
            "certificates.export",
            "certificates",
            ErrorCode::INVALID_ARGUMENT,
        ),
        ("certificates.list", "secrets", ErrorCode::INVALID_ARGUMENT),
    ] {
        let response = read(&service, operation, resource).await;
        assert_eq!(response.error.unwrap().code, code, "{operation} {resource}");
    }
    let unknown_field = change(
        &service,
        "certificates.generate",
        "certificates",
        "",
        json!({ "id": "x", "names": ["x.example"], "days": 1, "key": "nope" }),
    )
    .await;
    assert_eq!(
        unknown_field.error.unwrap().code,
        ErrorCode::INVALID_ARGUMENT
    );
}

async fn reminded(service: &ServiceDatabase, id: &str) -> Option<i32> {
    sqlx::query_scalar("SELECT reminded_days FROM certificates WHERE certificate_id = $1")
        .bind(id)
        .fetch_one(service.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn expiring_certificates_are_announced_once_per_threshold() {
    let Some((_database, service)) = database().await else {
        return;
    };
    let inventory = inventory(
        &service,
        Some(vault(&[&EnvelopeVault::generate_key().unwrap()])),
        None,
    );
    let scope = scope();
    let first = material("example.com");
    inventory
        .upload(cause(&scope), id("example.com"), &first.chain, &first.key)
        .await
        .unwrap();
    let lasting = self_signed(&["lasting.example".to_owned()], 365, Utc::now()).unwrap();
    inventory
        .upload(
            cause(&scope),
            id("lasting.example"),
            &lasting.chain,
            &lasting.key,
        )
        .await
        .unwrap();
    let now = Utc::now();
    let remind = |at| inventory.remind_expiring(cause(&scope), at);

    assert_eq!(remind(now).await.unwrap(), 1);
    assert_eq!(reminded(&service, "example.com").await, Some(30));
    assert_eq!(remind(now).await.unwrap(), 0);
    assert_eq!(remind(now + chrono::Duration::days(25)).await.unwrap(), 1);
    assert_eq!(reminded(&service, "example.com").await, Some(7));
    let expired = now + chrono::Duration::days(31);
    assert_eq!(remind(expired).await.unwrap(), 1);
    assert_eq!(remind(expired).await.unwrap(), 0);
    assert_eq!(reminded(&service, "example.com").await, Some(0));
    assert_eq!(reminded(&service, "lasting.example").await, None);

    let renewed = material("example.com");
    inventory
        .replace(
            cause(&scope),
            id("example.com"),
            None,
            &renewed.chain,
            &renewed.key,
        )
        .await
        .unwrap();
    assert_eq!(reminded(&service, "example.com").await, None);
    assert_eq!(remind(now).await.unwrap(), 1);
    assert_eq!(
        events(&service)
            .await
            .iter()
            .filter(|kind| *kind == "expiring")
            .count(),
        4
    );
}
