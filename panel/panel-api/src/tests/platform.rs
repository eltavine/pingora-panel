use super::*;
use chrono::{DateTime, Utc};
use panel_platform::{
    Capability, ProtocolRange, ServiceDescriptor, ServiceDirectory, ServiceListing, ServiceName,
};
use serde_json::Value;

struct Directory(ServiceListing);

#[async_trait]
impl ServiceDirectory for Directory {
    async fn list(&self) -> Result<ServiceListing> {
        Ok(self.0.clone())
    }
}

fn time(value: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(value)
        .unwrap()
        .with_timezone(&Utc)
}

fn services_request() -> Request<Body> {
    Request::builder()
        .uri("/api/v1/platform/services")
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn the_directory_lists_instances_with_protocols_and_capabilities() {
    let instance = ServiceDescriptor::new(
        ServiceName::new("config-service").unwrap(),
        "0.1.0",
        time("2026-10-03T05:00:00Z"),
    )
    .with_schema_version("10000")
    .with_protocol(ProtocolRange::up_to("pingora.panel.config.v1", 1).unwrap())
    .with_capability(Capability::new("config.publication", "1").unwrap());
    let listing = ServiceListing::new(time("2026-10-03T05:00:07.250Z"), vec![instance.clone()]);
    let router = router(
        ApiState::new(Arc::new(GatewayService::new(
            Arc::new(FakeGateway),
            Arc::new(IdentityCompiler),
        )))
        .with_directory(Arc::new(Directory(listing))),
    );
    let response = router.oneshot(services_request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 8192)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["observed_at"], "2026-10-03T05:00:07.250Z");
    let service = &body["services"][0];
    assert_eq!(service["service"], "config-service");
    assert_eq!(service["instance_id"], instance.instance_id().to_string());
    assert_eq!(service["schema_version"], "10000");
    assert_eq!(service["started_at"], "2026-10-03T05:00:00.000Z");
    assert_eq!(
        service["protocols"][0],
        serde_json::json!({"name": "pingora.panel.config.v1", "min_revision": 1, "max_revision": 1})
    );
    assert_eq!(
        service["capabilities"][0],
        serde_json::json!({"name": "config.publication", "version": "1"})
    );
}

#[tokio::test]
async fn an_unconfigured_directory_is_reported_as_unsupported() {
    let response = app().oneshot(services_request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}
