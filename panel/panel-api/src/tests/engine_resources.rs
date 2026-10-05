use super::*;
use panel_application::{
    CommandContext, EngineDiskUsage, EngineDiskUse, EngineNetwork, EngineNetworkList,
    EngineResourcesPort, EngineSubnet, EngineVolume, EngineVolumeList, PruneChoices, PruneItem,
    PruneKind, PruneOutcome, PrunePreview, PruneReport, RequestScope,
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

    async fn prune_preview(
        &self,
        _: RequestScope,
        _: String,
        choices: PruneChoices,
    ) -> Result<PrunePreview> {
        let mut items = vec![stopped()];
        if choices.named_volumes {
            items.push(PruneItem {
                kind: PruneKind::Volume,
                id: "orphan".into(),
                name: "orphan".into(),
                size_bytes: 2_048,
            });
        }
        Ok(PrunePreview {
            observed_at: Some(UNIX_EPOCH + Duration::from_secs(1_800_000_010)),
            reclaimable_bytes: items.iter().map(|item| item.size_bytes).sum(),
            items,
        })
    }

    async fn prune(
        &self,
        _: CommandContext,
        engine: String,
        _: PruneChoices,
        items: Vec<PruneItem>,
    ) -> Result<PruneReport> {
        if engine == "podman" {
            return Err(PanelError::precondition_failed(
                "the podman engine is disabled",
            ));
        }
        let outcomes: Vec<PruneOutcome> = items
            .into_iter()
            .map(|item| PruneOutcome {
                refusal: (item != stopped()).then(|| {
                    PanelError::precondition_failed("no longer what pruning would remove")
                }),
                item,
            })
            .collect();
        Ok(PruneReport {
            reclaimed_bytes: outcomes
                .iter()
                .filter(|outcome| outcome.refusal.is_none())
                .map(|outcome| outcome.item.size_bytes)
                .sum(),
            outcomes,
        })
    }
}

/// The stopped container pruning would remove.
fn stopped() -> PruneItem {
    PruneItem {
        kind: PruneKind::Container,
        id: "a1".into(),
        name: "cache".into(),
        size_bytes: 1_024,
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

#[tokio::test]
async fn pruning_shows_first_then_removes_what_still_goes() {
    let app = app(true);
    let path = "/api/v1/container-engines/docker";
    let (status, preview) = get(&app, &format!("{path}/prune-preview?named_volumes=true")).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["items"][0]["kind"], "container");
    assert_eq!(preview["items"][1]["kind"], "volume");
    assert_eq!(preview["reclaimable_bytes"], 3_072);
    let (_, plain) = get(&app, &format!("{path}/prune-preview")).await;
    assert_eq!(plain["items"].as_array().unwrap().len(), 1);

    let prune = |body: Value| {
        Request::post(format!("{path}/prune"))
            .header("content-type", "application/json")
            .header("x-actor", "ops")
            .header("idempotency-key", "prune-1")
            .header("x-deadline", "2099-01-01T00:00:00Z")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let send = |request: Request<Body>| {
        let app = app.clone();
        async move {
            let response = app.oneshot(request).await.unwrap();
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            (
                status,
                serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
            )
        }
    };
    let (status, report) = send(prune(serde_json::json!({
        "named_volumes": true,
        "items": preview["items"].clone()
    })))
    .await;
    assert_eq!(status, StatusCode::OK, "{report}");
    assert_eq!(report["reclaimed_bytes"], 1_024);
    assert_eq!(report["outcomes"][0]["error"], Value::Null);
    assert_eq!(
        report["outcomes"][1]["error"]["code"],
        "PRECONDITION_FAILED"
    );

    for body in [
        serde_json::json!({"items": [{"kind": "volume", "id": "../etc", "name": "x", "size_bytes": 0}]}),
        serde_json::json!({"items": [{"kind": "pod", "id": "x", "name": "x", "size_bytes": 0}]}),
        serde_json::json!({"items": (0..1001).map(|n| serde_json::json!({"kind": "volume", "id": format!("v{n}"), "name": "", "size_bytes": 0})).collect::<Vec<_>>()}),
    ] {
        let (status, _) = send(prune(body)).await;
        assert!(status.is_client_error(), "{status}");
    }
}
