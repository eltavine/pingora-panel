use super::*;
use panel_application::{HostFilesystem, HostNetworkDevice, HostPort, HostSummary, RequestScope};
use serde_json::Value;
use std::time::{Duration, UNIX_EPOCH};

struct Host;

fn filesystem(mountpoint: &str, available: f64) -> HostFilesystem {
    HostFilesystem {
        mountpoint: mountpoint.into(),
        device: format!("/dev/{}", mountpoint.trim_start_matches('/')),
        fstype: "ext4".into(),
        size_bytes: 100.0,
        available_bytes: available,
    }
}

#[async_trait]
impl HostPort for Host {
    async fn summary(&self, _scope: RequestScope) -> Result<HostSummary> {
        Ok(HostSummary {
            observed_at: Some(UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
            reporting: true,
            hostname: "web-1".into(),
            operating_system: "Ubuntu 24.04.2 LTS".into(),
            kernel_release: "6.8.0".into(),
            architecture: "x86_64".into(),
            host_time: Some(UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
            time_zone: "UTC".into(),
            uptime: Some(Duration::from_secs(86_400)),
            cpu_count: 4,
            cpu_usage: Some(0.25),
            load1: 0.5,
            load5: 0.75,
            load15: 1.0,
            memory_total_bytes: 8e9,
            memory_available_bytes: 2e9,
            filesystems: vec![
                filesystem("/var", 3.0),
                filesystem("/data", 10.0),
                filesystem("/", 50.0),
            ],
            network_devices: vec![HostNetworkDevice {
                device: "eth0".into(),
                receive_bytes_per_second: 1200.0,
                transmit_bytes_per_second: 300.0,
            }],
        })
    }
}

async fn get(app: &axum::Router) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(Request::get("/api/v1/host").body(Body::empty()).unwrap())
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
async fn host_figures_warn_about_full_filesystems() {
    let app = router(
        ApiState::new(Arc::new(GatewayService::new(
            Arc::new(FakeGateway),
            Arc::new(IdentityCompiler),
        )))
        .with_host(Arc::new(Host)),
    );
    let (status, host) = get(&app).await;
    assert_eq!(status, StatusCode::OK, "{host}");
    assert_eq!(host["hostname"], "web-1");
    assert_eq!(host["uptime_seconds"], 86_400);
    assert_eq!(host["observed_at"], "2027-01-15T08:00:00Z");
    let levels: Vec<_> = host["filesystems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|filesystem| {
            (
                filesystem["mountpoint"].clone(),
                filesystem["level"].clone(),
            )
        })
        .collect();
    assert_eq!(
        levels,
        [
            ("/var".into(), "critical".into()),
            ("/data".into(), "warning".into()),
            ("/".into(), "ok".into()),
        ]
    );
    assert_eq!(host["filesystems"][2]["used_ratio"], 0.5);
    assert_eq!(host["network_devices"][0]["device"], "eth0");
}

#[tokio::test]
async fn without_a_source_the_host_is_unavailable() {
    let app = router(ApiState::new(Arc::new(GatewayService::new(
        Arc::new(FakeGateway),
        Arc::new(IdentityCompiler),
    ))));
    assert_eq!(get(&app).await.0, StatusCode::SERVICE_UNAVAILABLE);
}
