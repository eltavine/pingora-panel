use super::*;
use chrono::Utc;
use panel_application::{RequestScope, TlsProbe, TlsProbeReport, TlsProbeTarget};
use panel_certificates::self_signed;
use panel_config_api::{
    ApplyOutcome, ApplyRequest, ConfigurationChange, ConfigurationOutput, ConfigurationPort,
    ConfigurationQuery, DraftInfo, ModelQuery,
};
use rustls_pki_types::{pem::PemObject, CertificateDer};
use serde_json::{json, Value};
use std::{sync::Mutex, time::Duration};

struct Listeners;

#[async_trait]
impl ConfigurationPort for Listeners {
    async fn read(
        &self,
        _scope: RequestScope,
        query: ConfigurationQuery,
    ) -> Result<ConfigurationOutput> {
        let ConfigurationQuery::Model(ModelQuery::Listener { id }) = query else {
            return Err(PanelError::unavailable("only listeners are read here"));
        };
        let (listener, etag) = match id.as_str() {
            "https" => (
                json!({"id": "https", "address": "0.0.0.0:8443", "tls_profile_id": "edge"}),
                "\"l1\"",
            ),
            "http" => (
                json!({"id": "http", "address": "0.0.0.0:8080", "tls_profile_id": null}),
                "\"l2\"",
            ),
            _ => return Err(PanelError::not_found("there is no such listener")),
        };
        Ok(ConfigurationOutput {
            content: serde_json::to_vec(&listener).unwrap(),
            etag: Some(etag.into()),
            draft: DraftInfo::default(),
        })
    }

    async fn change(
        &self,
        _context: CommandContext,
        _change: ConfigurationChange,
    ) -> Result<ConfigurationOutput> {
        Err(PanelError::unavailable("read only"))
    }

    async fn apply(
        &self,
        _context: CommandContext,
        _request: ApplyRequest,
    ) -> Result<ApplyOutcome> {
        Err(PanelError::unavailable("read only"))
    }
}

#[derive(Default)]
struct Probe {
    targets: Mutex<Vec<TlsProbeTarget>>,
}

#[async_trait]
impl TlsProbe for Probe {
    async fn probe(&self, _scope: RequestScope, target: TlsProbeTarget) -> Result<TlsProbeReport> {
        self.targets.lock().unwrap().push(target);
        let material = self_signed(&["shop.example".into()], 90, Utc::now()).unwrap();
        let leaf = CertificateDer::from_pem_slice(material.chain.as_bytes()).unwrap();
        let mut report = TlsProbeReport::default();
        report.protocol = "TLSv1.3".into();
        report.cipher_suite = "TLS13_AES_256_GCM_SHA384".into();
        report.alpn = Some("h2".into());
        report.handshake = Duration::from_millis(12);
        report.chain = vec![leaf.to_vec()];
        report.versions = vec![("TLSv1.2".into(), false), ("TLSv1.3".into(), true)];
        Ok(report)
    }
}

fn app(probe: Option<Arc<Probe>>) -> axum::Router {
    let state = ApiState::new(Arc::new(GatewayService::new(
        Arc::new(FakeGateway),
        Arc::new(IdentityCompiler),
    )))
    .with_configuration(Arc::new(Listeners));
    router(match probe {
        Some(probe) => state.with_tls_probe(probe),
        None => state,
    })
}

async fn check(app: &axum::Router, body: Value) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/tls-checks")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn checks_probe_the_listener_for_the_host() {
    let probe = Arc::new(Probe::default());
    let app = app(Some(Arc::clone(&probe)));

    let (status, checked) = check(&app, json!({"listener": "https", "host": "Shop.Example"})).await;
    assert_eq!(status, StatusCode::OK, "{checked}");
    let targets = probe.targets.lock().unwrap().clone();
    assert_eq!(
        targets,
        [TlsProbeTarget {
            address: "127.0.0.1:8443".parse().unwrap(),
            server_name: "shop.example".into(),
        }]
    );
    assert_eq!(checked["address"], "127.0.0.1:8443");
    assert_eq!(checked["protocol"], "TLSv1.3");
    assert_eq!(checked["handshake_ms"], 12);
    assert_eq!(checked["covers_host"], true);
    assert_eq!(checked["certificate"]["names"], json!(["shop.example"]));
    assert_eq!(checked["certificate_status"], "valid");
    assert_eq!(
        checked["versions"],
        json!([{"version": "TLSv1.2", "accepted": false}, {"version": "TLSv1.3", "accepted": true}])
    );

    for (body, expected) in [
        (
            json!({"listener": "http", "host": "shop.example"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"listener": "missing", "host": "shop.example"}),
            StatusCode::NOT_FOUND,
        ),
        (
            json!({"listener": "https", "host": "*.shop.example"}),
            StatusCode::BAD_REQUEST,
        ),
        (
            json!({"listener": "https"}),
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ] {
        let (status, problem) = check(&app, body.clone()).await;
        assert_eq!(status, expected, "{body}: {problem}");
    }
    assert_eq!(probe.targets.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn without_a_probe_checks_are_unavailable() {
    let (status, _) = check(
        &app(None),
        json!({"listener": "https", "host": "shop.example"}),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}
