use super::*;
use chrono::Utc;
use panel_application::{
    CertificateChange, CertificateOutput, CertificatePort, CertificateRead, RequestScope,
};
use panel_certificates::{self_signed, Certificate, CertificateId, CertificateSource};
use serde_json::{json, Value};
use std::sync::Mutex;

type Call = (String, String, Option<String>, Value);

#[derive(Default)]
struct FakeInventory {
    calls: Mutex<Vec<Call>>,
}

fn stored(version: u64) -> Certificate {
    let material = self_signed(
        &["example.com".into(), "*.example.com".into()],
        90,
        Utc::now(),
    )
    .unwrap();
    Certificate {
        id: CertificateId::new("example.com").unwrap(),
        source: CertificateSource::Uploaded,
        details: material.details,
        chain: material.chain,
        version,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

fn output(certificate: Certificate) -> CertificateOutput {
    CertificateOutput {
        etag: Some(format!("\"{}\"", certificate.version)),
        content: serde_json::to_vec(&certificate).unwrap(),
    }
}

#[async_trait]
impl CertificatePort for FakeInventory {
    async fn read(&self, _scope: RequestScope, read: CertificateRead) -> Result<CertificateOutput> {
        self.calls.lock().unwrap().push((
            read.operation.clone(),
            read.resource.clone(),
            None,
            Value::Null,
        ));
        match (read.operation.as_str(), read.resource.as_str()) {
            ("certificates.list", "certificates") => Ok(CertificateOutput {
                content: serde_json::to_vec(&[stored(1)]).unwrap(),
                etag: None,
            }),
            ("certificates.get", "certificates/example.com") => Ok(output(stored(3))),
            _ => Err(PanelError::not_found("there is no such certificate")),
        }
    }

    async fn change(
        &self,
        context: CommandContext,
        change: CertificateChange,
    ) -> Result<CertificateOutput> {
        assert_eq!(context.actor(), "operator");
        let content = if change.content.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&change.content).unwrap()
        };
        self.calls.lock().unwrap().push((
            change.operation.clone(),
            change.resource.clone(),
            change.if_match.clone(),
            content,
        ));
        Ok(match change.operation.as_str() {
            "certificates.delete" => CertificateOutput {
                content: Vec::new(),
                etag: None,
            },
            _ => output(stored(1)),
        })
    }
}

fn app(inventory: Option<Arc<FakeInventory>>) -> axum::Router {
    let state = ApiState::new(Arc::new(GatewayService::new(
        Arc::new(FakeGateway),
        Arc::new(IdentityCompiler),
    )));
    router(match inventory {
        Some(inventory) => state.with_certificates(inventory),
        None => state,
    })
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
async fn certificate_requests_map_onto_the_inventory_port() {
    let inventory = Arc::new(FakeInventory::default());
    let app = app(Some(Arc::clone(&inventory)));

    let (status, _, listed) = send(&app, request("GET", "/api/v1/certificates", None, None)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed[0]["id"], "example.com");
    assert_eq!(listed[0]["status"], "valid");
    assert_eq!(listed[0]["names"], json!(["example.com", "*.example.com"]));

    let (status, etag, created) = send(
        &app,
        request(
            "POST",
            "/api/v1/certificates",
            None,
            Some(json!({ "source": "upload", "id": "example.com", "chain": "C", "key": "K" })),
        ),
    )
    .await;
    assert_eq!(
        (status, etag.as_deref()),
        (StatusCode::CREATED, Some("\"1\""))
    );
    assert_eq!(created["source"], "uploaded");
    send(
        &app,
        request(
            "POST",
            "/api/v1/certificates",
            None,
            Some(json!({ "source": "self_signed", "id": "internal", "names": ["a.example"], "days": 30 })),
        ),
    )
    .await;
    let (status, _, _) = send(
        &app,
        request(
            "POST",
            "/api/v1/certificates",
            None,
            Some(json!({ "source": "acme-v0", "id": "x" })),
        ),
    )
    .await;
    assert!(status.is_client_error());

    let replace = json!({ "chain": "C2", "key": "K2" });
    let (status, _, _) = send(
        &app,
        request(
            "PUT",
            "/api/v1/certificates/example.com",
            None,
            Some(replace.clone()),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::PRECONDITION_REQUIRED);
    let (status, _, _) = send(
        &app,
        request(
            "PUT",
            "/api/v1/certificates/example.com",
            Some("\"3\""),
            Some(replace),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _, _) = send(
        &app,
        request(
            "DELETE",
            "/api/v1/certificates/example.com",
            Some("\"1\""),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    let calls = inventory.calls.lock().unwrap().clone();
    let changes: Vec<_> = calls
        .iter()
        .filter(|(operation, ..)| operation != "certificates.list")
        .cloned()
        .collect();
    assert_eq!(
        changes,
        [
            (
                "certificates.upload".to_owned(),
                "certificates".to_owned(),
                None,
                json!({ "id": "example.com", "chain": "C", "key": "K" })
            ),
            (
                "certificates.generate".into(),
                "certificates".into(),
                None,
                json!({ "id": "internal", "names": ["a.example"], "days": 30 })
            ),
            (
                "certificates.replace".into(),
                "certificates/example.com".into(),
                Some("\"3\"".into()),
                json!({ "chain": "C2", "key": "K2" })
            ),
            (
                "certificates.delete".into(),
                "certificates/example.com".into(),
                Some("\"1\"".into()),
                Value::Null
            ),
        ]
    );
}

#[tokio::test]
async fn coverage_names_the_hosts_a_certificate_covers() {
    let app = app(Some(Arc::new(FakeInventory::default())));

    let (status, _, coverage) = send(
        &app,
        request(
            "GET",
            "/api/v1/certificates/example.com/coverage?hosts=www.example.com,%20example.org,b%C3%BCcher.example.com",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(coverage["status"], "valid");
    assert_eq!(
        coverage["hosts"],
        json!([
            { "host": "www.example.com", "covered": true },
            { "host": "example.org", "covered": false },
            { "host": "xn--bcher-kva.example.com", "covered": true },
        ])
    );
    for hosts in ["", "not%20a%20host"] {
        let (status, _, _) = send(
            &app,
            request(
                "GET",
                &format!("/api/v1/certificates/example.com/coverage?hosts={hosts}"),
                None,
                None,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{hosts}");
    }
    let (status, _, _) = send(
        &app,
        request(
            "GET",
            "/api/v1/certificates/missing/coverage?hosts=a.example",
            None,
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn inspections_check_material_without_storing_it() {
    let inventory = Arc::new(FakeInventory::default());
    let app = app(Some(Arc::clone(&inventory)));
    let material = self_signed(&["example.com".into()], 30, Utc::now()).unwrap();
    let other = self_signed(&["example.com".into()], 30, Utc::now()).unwrap();

    let (status, _, chain_only) = send(
        &app,
        request(
            "POST",
            "/api/v1/certificate-inspections",
            None,
            Some(json!({ "chain": material.chain })),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(chain_only["key_matches"], false);
    assert_eq!(chain_only["names"], json!(["example.com"]));
    let (_, _, with_key) = send(
        &app,
        request(
            "POST",
            "/api/v1/certificate-inspections",
            None,
            Some(json!({ "chain": material.chain, "key": *material.key })),
        ),
    )
    .await;
    assert_eq!(with_key["key_matches"], true);
    let (status, _, problem) = send(
        &app,
        request(
            "POST",
            "/api/v1/certificate-inspections",
            None,
            Some(json!({ "chain": material.chain, "key": *other.key })),
        ),
    )
    .await;
    assert!(status.is_client_error());
    assert!(problem["detail"]
        .as_str()
        .unwrap()
        .contains("does not belong"));
    assert!(inventory.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn without_an_inventory_certificates_are_unavailable() {
    let (status, _, _) = send(
        &app(None),
        request("GET", "/api/v1/certificates", None, None),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}
