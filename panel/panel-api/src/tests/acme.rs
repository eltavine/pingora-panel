use super::*;
use panel_application::RequestScope;
use panel_certificate_api::{
    self as api, CertificateChange, CertificateCommand, CertificateOutput, CertificatePort,
    CertificateQuery,
};
use panel_domain::CertificateId;
use serde_json::{json, Value};
use std::sync::Mutex;

type Call = (String, String, Option<String>, Option<CertificateCommand>);

#[derive(Default)]
struct FakeAutomation {
    calls: Mutex<Vec<Call>>,
}

fn account() -> Value {
    json!({
        "id": "letsencrypt",
        "directory": "https://acme-v02.api.letsencrypt.org/directory",
        "ca_bundle": null,
        "contact": ["ops@example.com"],
        "external_account_key_id": null,
        "url": "https://acme-v02.api.letsencrypt.org/acme/acct/1",
        "version": 1,
        "created_at": "2026-10-01T00:00:00Z",
        "updated_at": "2026-10-01T00:00:00Z",
    })
}

fn automatic(state: &str) -> Value {
    json!({
        "id": "example.com",
        "account": "letsencrypt",
        "names": ["example.com", "www.example.com"],
        "challenge": "http-01",
        "dns_provider": null,
        "state": state,
        "renew_after": "2026-10-01T00:00:00Z",
        "renewal_explanation_url": null,
        "failures": 0,
        "last_error": null,
        "version": 2,
        "created_at": "2026-10-01T00:00:00Z",
        "updated_at": "2026-10-01T00:00:00Z",
    })
}

fn provider() -> Value {
    json!({
        "id": "primary-ns",
        "kind": "rfc2136",
        "rfc2136": {
            "server": "ns1.example.com:53",
            "zones": ["example.com"],
            "key_name": "acme-update",
            "algorithm": "hmac-sha256",
        },
        "propagation_seconds": 30,
        "version": 2,
        "created_at": "2026-10-01T00:00:00Z",
        "updated_at": "2026-10-02T00:00:00Z",
    })
}

fn output(value: Value, etag: Option<&str>) -> CertificateOutput {
    CertificateOutput {
        content: serde_json::to_vec(&value).unwrap(),
        etag: etag.map(str::to_owned),
    }
}

#[async_trait]
impl CertificatePort for FakeAutomation {
    async fn read(
        &self,
        _scope: RequestScope,
        query: CertificateQuery,
    ) -> Result<CertificateOutput> {
        let (operation, resource) = (query.operation(), query.resource());
        self.calls
            .lock()
            .unwrap()
            .push((operation.into(), resource.clone(), None, None));
        match (operation, resource.as_str()) {
            ("acme.accounts.list", "acme-accounts") => Ok(output(json!([account()]), None)),
            ("acme.accounts.get", "acme-accounts/letsencrypt") => {
                Ok(output(account(), Some("\"1\"")))
            }
            ("acme.certificates.list", "acme-certificates") => {
                Ok(output(json!([automatic("issued")]), None))
            }
            ("acme.certificates.get", "acme-certificates/example.com") => {
                Ok(output(automatic("issued"), Some("\"2\"")))
            }
            ("acme.dns_providers.list", "dns-providers") => Ok(output(json!([provider()]), None)),
            ("acme.dns_providers.get", "dns-providers/primary-ns") => {
                Ok(output(provider(), Some("\"2\"")))
            }
            _ => Err(PanelError::not_found("there is no such resource")),
        }
    }

    async fn change(
        &self,
        context: CommandContext,
        change: CertificateChange,
    ) -> Result<CertificateOutput> {
        assert_eq!(context.actor(), "operator");
        let operation = change.command.operation();
        self.calls.lock().unwrap().push((
            operation.into(),
            change.command.resource(),
            change.if_match.clone(),
            Some(change.command),
        ));
        Ok(match operation {
            "acme.accounts.create" => output(account(), Some("\"1\"")),
            "acme.certificates.create" => output(automatic("pending"), Some("\"1\"")),
            "acme.certificates.renew" => output(automatic("issued"), Some("\"2\"")),
            "acme.dns_providers.create" | "acme.dns_providers.update" => {
                output(provider(), Some("\"2\""))
            }
            _ => output(Value::Null, None),
        })
    }
}

fn app(automation: &Arc<FakeAutomation>) -> axum::Router {
    let state = ApiState::new(Arc::new(GatewayService::new(
        Arc::new(FakeGateway),
        Arc::new(IdentityCompiler),
    )));
    router(state.with_certificates(Arc::clone(automation) as Arc<dyn CertificatePort>))
}

fn request(method: &str, uri: &str, if_match: Option<&str>, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("x-actor", "operator")
        .header("x-deadline", "2099-01-01T00:00:00Z")
        .header("idempotency-key", format!("{method}-{uri}"));
    if let Some(if_match) = if_match {
        builder = builder.header(header::IF_MATCH, if_match);
    }
    match body {
        Some(body) => builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

async fn send(app: &axum::Router, request: Request<Body>) -> (StatusCode, Option<String>, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let etag = response
        .headers()
        .get(header::ETAG)
        .map(|value| value.to_str().unwrap().to_owned());
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, etag, body)
}

#[tokio::test]
async fn acme_accounts_map_onto_the_automation_port() {
    let automation = Arc::new(FakeAutomation::default());
    let app = app(&automation);

    let (status, _, listed) = send(&app, request("GET", "/api/v1/acme-accounts", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed[0]["id"], "letsencrypt");
    assert_eq!(listed[0]["etag"], "\"1\"");

    let registration = json!({
        "id": "letsencrypt",
        "directory": "https://acme-v02.api.letsencrypt.org/directory",
        "contact": ["ops@example.com"],
        "terms_of_service_agreed": true,
        "external_account": {"key_id": "kid-1", "mac_key": "c2VjcmV0"},
    });
    let (status, etag, created) = send(
        &app,
        request(
            "POST",
            "/api/v1/acme-accounts",
            None,
            Some(registration.clone()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(etag.as_deref(), Some("\"1\""));
    assert_eq!(
        created["url"],
        "https://acme-v02.api.letsencrypt.org/acme/acct/1"
    );
    assert!(!created.to_string().contains("c2VjcmV0"));

    let (status, _, _) = send(
        &app,
        request(
            "POST",
            "/api/v1/acme-accounts",
            None,
            Some(json!({"id": "x", "directory": "https://ca.example/dir", "terms_of_service_agreed": true, "key": "pem"})),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "unknown fields are refused"
    );

    let (status, etag, _) = send(
        &app,
        request("GET", "/api/v1/acme-accounts/letsencrypt", None, None),
    )
    .await;
    assert_eq!((status, etag.as_deref()), (StatusCode::OK, Some("\"1\"")));
    let (status, _, _) = send(
        &app,
        request("DELETE", "/api/v1/acme-accounts/letsencrypt", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::PRECONDITION_REQUIRED);
    let (status, _, _) = send(
        &app,
        request(
            "DELETE",
            "/api/v1/acme-accounts/letsencrypt",
            Some("\"1\""),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let calls = automation.calls.lock().unwrap().clone();
    let operations: Vec<(&str, &str, Option<&str>)> = calls
        .iter()
        .map(|(operation, resource, if_match, _)| {
            (operation.as_str(), resource.as_str(), if_match.as_deref())
        })
        .collect();
    assert_eq!(
        operations,
        [
            ("acme.accounts.list", "acme-accounts", None),
            ("acme.accounts.create", "acme-accounts", None),
            ("acme.accounts.get", "acme-accounts/letsencrypt", None),
            (
                "acme.accounts.delete",
                "acme-accounts/letsencrypt",
                Some("\"1\"")
            ),
        ]
    );
    assert_eq!(
        calls[1].3,
        Some(CertificateCommand::CreateAccount {
            account: api::NewAccount {
                id: api::AccountId::new("letsencrypt").unwrap(),
                directory: "https://acme-v02.api.letsencrypt.org/directory".into(),
                ca_bundle: None,
                contact: vec!["ops@example.com".into()],
                terms_of_service_agreed: true,
                external_account: Some(api::ExternalAccount {
                    key_id: "kid-1".into(),
                    mac_key: api::Secret::new("c2VjcmV0"),
                }),
            },
        })
    );
}

#[tokio::test]
async fn automatic_certificates_map_onto_the_automation_port() {
    let automation = Arc::new(FakeAutomation::default());
    let app = app(&automation);

    let order = json!({
        "id": "example.com",
        "account": "letsencrypt",
        "names": ["example.com", "www.example.com"],
    });
    let (status, etag, created) = send(
        &app,
        request(
            "POST",
            "/api/v1/acme-certificates",
            None,
            Some(order.clone()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(etag.as_deref(), Some("\"1\""));
    assert_eq!(created["state"], "pending");
    assert_eq!(created["challenge"], "http-01");

    let (status, _, listed) = send(
        &app,
        request("GET", "/api/v1/acme-certificates", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed[0]["etag"], "\"2\"");
    let (status, etag, read) = send(
        &app,
        request("GET", "/api/v1/acme-certificates/example.com", None, None),
    )
    .await;
    assert_eq!((status, etag.as_deref()), (StatusCode::OK, Some("\"2\"")));
    assert_eq!(read["names"], json!(["example.com", "www.example.com"]));

    let (status, _, renewed) = send(
        &app,
        request(
            "POST",
            "/api/v1/acme-certificates/example.com/renewals",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(renewed["id"], "example.com");

    let (status, _, _) = send(
        &app,
        request(
            "DELETE",
            "/api/v1/acme-certificates/example.com",
            Some("\"2\""),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let calls = automation.calls.lock().unwrap().clone();
    let operations: Vec<(&str, &str, Option<&str>)> = calls
        .iter()
        .map(|(operation, resource, if_match, _)| {
            (operation.as_str(), resource.as_str(), if_match.as_deref())
        })
        .collect();
    assert_eq!(
        operations,
        [
            ("acme.certificates.create", "acme-certificates", None),
            ("acme.certificates.list", "acme-certificates", None),
            (
                "acme.certificates.get",
                "acme-certificates/example.com",
                None
            ),
            (
                "acme.certificates.renew",
                "acme-certificates/example.com",
                None
            ),
            (
                "acme.certificates.delete",
                "acme-certificates/example.com",
                Some("\"2\"")
            ),
        ]
    );
    assert_eq!(
        calls[0].3,
        Some(CertificateCommand::CreateAutomaticCertificate {
            certificate: api::NewAutomaticCertificate {
                id: CertificateId::new("example.com").unwrap(),
                account: api::AccountId::new("letsencrypt").unwrap(),
                names: vec!["example.com".into(), "www.example.com".into()],
                challenge: api::Challenge::Http01,
                dns_provider: None,
                dns_plugin: None,
            },
        })
    );
}

#[tokio::test]
async fn dns_01_certificates_can_name_a_plugin_to_publish_their_records() {
    let automation = Arc::new(FakeAutomation::default());
    let app = app(&automation);
    let order = json!({
        "id": "example.com",
        "account": "letsencrypt",
        "names": ["*.example.com"],
        "challenge": "dns-01",
        "dns_plugin": "cloud-dns",
    });
    let (status, _, _) = send(
        &app,
        request("POST", "/api/v1/acme-certificates", None, Some(order)),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let calls = automation.calls.lock().unwrap();
    match &calls[0].3 {
        Some(CertificateCommand::CreateAutomaticCertificate { certificate }) => {
            assert_eq!(certificate.challenge, api::Challenge::Dns01);
            assert_eq!(certificate.dns_plugin.as_deref(), Some("cloud-dns"));
            assert_eq!(certificate.dns_provider, None);
        }
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn dns_providers_map_onto_the_automation_port_without_their_secret() {
    let automation = Arc::new(FakeAutomation::default());
    let app = app(&automation);

    let (status, _, listed) = send(&app, request("GET", "/api/v1/dns-providers", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed[0]["rfc2136"]["zones"], json!(["example.com"]));
    assert_eq!(listed[0]["etag"], "\"2\"");

    let created_body = json!({
        "id": "primary-ns",
        "kind": "rfc2136",
        "rfc2136": {
            "server": "ns1.example.com:53",
            "zones": ["example.com"],
            "key_name": "acme-update",
            "algorithm": "hmac-sha256",
        },
        "secret": "c2VjcmV0",
    });
    let (status, etag, created) = send(
        &app,
        request(
            "POST",
            "/api/v1/dns-providers",
            None,
            Some(created_body.clone()),
        ),
    )
    .await;
    assert_eq!(
        (status, etag.as_deref()),
        (StatusCode::CREATED, Some("\"2\""))
    );
    assert!(!created.to_string().contains("c2VjcmV0"));

    let (status, _, _) = send(
        &app,
        request(
            "POST",
            "/api/v1/dns-providers",
            None,
            Some(json!({"id": "x", "kind": "cloudflare", "rfc2136": created_body["rfc2136"], "secret": "c2VjcmV0"})),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "unknown kinds are refused"
    );

    let change = json!({
        "rfc2136": {
            "server": "ns2.example.com:53",
            "zones": ["example.com"],
            "key_name": "acme-update",
            "algorithm": "hmac-sha512",
        },
        "propagation_seconds": 60,
    });
    let (status, _, _) = send(
        &app,
        request(
            "PUT",
            "/api/v1/dns-providers/primary-ns",
            None,
            Some(change.clone()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::PRECONDITION_REQUIRED);
    let (status, _, _) = send(
        &app,
        request(
            "PUT",
            "/api/v1/dns-providers/primary-ns",
            Some("\"2\""),
            Some(change.clone()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, etag, _) = send(
        &app,
        request("GET", "/api/v1/dns-providers/primary-ns", None, None),
    )
    .await;
    assert_eq!((status, etag.as_deref()), (StatusCode::OK, Some("\"2\"")));
    let (status, _, _) = send(
        &app,
        request(
            "DELETE",
            "/api/v1/dns-providers/primary-ns",
            Some("\"2\""),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let calls = automation.calls.lock().unwrap().clone();
    let operations: Vec<(&str, &str, Option<&str>)> = calls
        .iter()
        .map(|(operation, resource, if_match, _)| {
            (operation.as_str(), resource.as_str(), if_match.as_deref())
        })
        .collect();
    assert_eq!(
        operations,
        [
            ("acme.dns_providers.list", "dns-providers", None),
            ("acme.dns_providers.create", "dns-providers", None),
            (
                "acme.dns_providers.update",
                "dns-providers/primary-ns",
                Some("\"2\"")
            ),
            ("acme.dns_providers.get", "dns-providers/primary-ns", None),
            (
                "acme.dns_providers.delete",
                "dns-providers/primary-ns",
                Some("\"2\"")
            ),
        ]
    );
    let settings = |server: &str, algorithm| api::Rfc2136Config {
        server: server.into(),
        zones: vec!["example.com".into()],
        key_name: "acme-update".into(),
        algorithm,
        ttl: None,
    };
    assert_eq!(
        calls[1].3,
        Some(CertificateCommand::CreateDnsProvider {
            provider: api::NewDnsProvider {
                id: "primary-ns".into(),
                kind: api::DnsProviderKind::Rfc2136,
                rfc2136: settings("ns1.example.com:53", api::TsigAlgorithm::HmacSha256),
                secret: api::Secret::new("c2VjcmV0"),
                propagation_seconds: None,
            },
        })
    );
    assert_eq!(
        calls[2].3,
        Some(CertificateCommand::UpdateDnsProvider {
            id: "primary-ns".into(),
            change: api::DnsProviderChange {
                rfc2136: settings("ns2.example.com:53", api::TsigAlgorithm::HmacSha512),
                secret: None,
                propagation_seconds: Some(60),
            },
        })
    );
}
