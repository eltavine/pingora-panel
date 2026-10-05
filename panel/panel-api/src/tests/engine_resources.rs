use super::*;
use panel_application::{
    EngineDiskUsage, EngineDiskUse, EngineNetwork, EngineNetworkList, EngineResourcesPort,
    EngineSubnet, EngineVolume, EngineVolumeList, RequestScope,
};
use serde_json::Value;
use std::time::{Duration, UNIX_EPOCH};

/// A Compose project's network and volume.
struct Resources;

#[async_trait]
impl EngineResourcesPort for Resources {
    async fn networks(&self, _: RequestScope, _: String) -> Result<EngineNetworkList> {
        Ok(EngineNetworkList {
            observed_at: Some(UNIX_EPOCH + Duration::from_secs(1_800_000_010)),
            networks: vec![EngineNetwork {
                id: "n3".into(),
                name: "shop_default".into(),
                driver: "bridge".into(),
                scope: "local".into(),
                created: Some(UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
                subnets: vec![EngineSubnet {
                    subnet: "172.18.0.0/16".into(),
                    gateway: Some("172.18.0.1".into()),
                }],
                containers: 2,
                compose_project: Some("shop".into()),
                ..EngineNetwork::default()
            }],
        })
    }

    async fn volumes(&self, _: RequestScope, engine: String) -> Result<EngineVolumeList> {
        if engine == "podman" {
            return Err(PanelError::precondition_failed(
                "the podman engine is disabled",
            ));
        }
        Ok(EngineVolumeList {
            observed_at: None,
            volumes: vec![EngineVolume {
                name: "shop_html".into(),
                driver: "local".into(),
                mountpoint: "/var/lib/docker/volumes/shop_html/_data".into(),
                containers: 1,
                ..EngineVolume::default()
            }],
        })
    }

    async fn disk_usage(&self, _: RequestScope, _: String) -> Result<EngineDiskUsage> {
        Ok(EngineDiskUsage {
            observed_at: Some(UNIX_EPOCH + Duration::from_secs(1_800_000_010)),
            images: EngineDiskUse {
                total: 5,
                active: 2,
                size_bytes: 1_000_000_000,
                reclaimable_bytes: 300_000_000,
            },
            build_cache: EngineDiskUse {
                total: 3,
                active: 1,
                size_bytes: 785,
                reclaimable_bytes: 700,
            },
            ..EngineDiskUsage::default()
        })
    }
}

fn app(resources: bool) -> axum::Router {
    let state = ApiState::new(Arc::new(GatewayService::new(
        Arc::new(FakeGateway),
        Arc::new(IdentityCompiler),
    )));
    router(if resources {
        state.with_engine_resources(Arc::new(Resources))
    } else {
        state
    })
}

async fn get(app: &axum::Router, path: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn networks_and_volumes_are_listed() {
    let app = app(true);
    let (status, networks) = get(&app, "/api/v1/container-engines/docker/networks").await;
    assert_eq!(status, StatusCode::OK, "{networks}");
    let shop = &networks["networks"][0];
    assert_eq!(shop["name"], "shop_default");
    assert_eq!(shop["created"], "2027-01-15T08:00:00Z");
    assert_eq!(shop["subnets"][0]["gateway"], "172.18.0.1");
    assert_eq!(shop["compose_project"], "shop");
    assert_eq!(shop["containers"], 2);

    let (status, volumes) = get(&app, "/api/v1/container-engines/docker/volumes").await;
    assert_eq!(status, StatusCode::OK, "{volumes}");
    assert_eq!(
        volumes["volumes"][0]["mountpoint"],
        "/var/lib/docker/volumes/shop_html/_data"
    );
    assert_eq!(volumes["volumes"][0]["compose_project"], Value::Null);
    let (status, problem) = get(&app, "/api/v1/container-engines/podman/volumes").await;
    assert_eq!(problem["code"], "PRECONDITION_FAILED", "{status}");
    let (status, _) = get(&app, "/api/v1/container-engines/Docker/volumes").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn without_the_agent_networks_are_unsupported() {
    let (status, problem) = get(&app(false), "/api/v1/container-engines/docker/networks").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert_eq!(problem["code"], "UNSUPPORTED_CAPABILITY");
}

#[tokio::test]
async fn disk_use_is_read() {
    let (status, usage) = get(&app(true), "/api/v1/container-engines/docker/disk-usage").await;
    assert_eq!(status, StatusCode::OK, "{usage}");
    assert_eq!(usage["observed_at"], "2027-01-15T08:00:10Z");
    assert_eq!(usage["images"]["total"], 5);
    assert_eq!(usage["images"]["reclaimable_bytes"], 300_000_000);
    assert_eq!(usage["build_cache"]["active"], 1);
    assert_eq!(usage["containers"]["size_bytes"], 0);
}
