#![forbid(unsafe_code)]

use async_trait::async_trait;
use chrono::Utc;
use panel_errors::ErrorCode;
use panel_health::{
    CheckOutcome, ComponentType, HealthCheck, HealthRegistry, HealthReport, HealthWatch, Impact,
    ServiceIdentity,
};
use panel_platform::{Capability, ProtocolRange, ServiceDescriptor, ServiceName};
use panel_service::{
    describe_peer, negotiate_with_peer, ops_router, probe_http, publish_grpc_health,
    ServiceInfoService, LIVENESS_PATH, READINESS_PATH,
};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::net::TcpListener;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::transport::{Channel, Server};
use tonic_health::pb::{
    health_check_response::ServingStatus, health_client::HealthClient, HealthCheckRequest,
};

const PLATFORM: &str = "pingora.panel.platform.v1";
const TIMEOUT: Duration = Duration::from_secs(2);

struct Database(bool);

#[async_trait]
impl HealthCheck for Database {
    fn component(&self) -> &str {
        "postgresql"
    }
    fn component_type(&self) -> ComponentType {
        ComponentType::Datastore
    }
    async fn check(&self) -> CheckOutcome {
        if self.0 {
            CheckOutcome::pass()
        } else {
            CheckOutcome::fail("unreachable")
        }
    }
}

async fn health(healthy: bool) -> HealthWatch {
    HealthWatch::fixed(
        HealthRegistry::new(ServiceIdentity::new("config-service", "0.1.0"))
            .register(Arc::new(Database(healthy)), Impact::Required)
            .evaluate()
            .await,
    )
}

async fn serve_ops(health: HealthWatch) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, ops_router(health)).await });
    address
}

async fn get(address: SocketAddr, path: &str) -> (u16, String, String, serde_json::Value) {
    let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: test\r\nConnection: close\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).await.unwrap();
    let (head, body) = raw.split_once("\r\n\r\n").unwrap();
    let status = head[9..12].parse().unwrap();
    let header = |name: &str| {
        head.lines()
            .find_map(|line| {
                let (key, value) = line.split_once(": ")?;
                key.eq_ignore_ascii_case(name).then(|| value.to_owned())
            })
            .unwrap_or_default()
    };
    (
        status,
        header("content-type"),
        header("cache-control"),
        serde_json::from_str(body).unwrap(),
    )
}

#[tokio::test]
async fn operational_endpoints_report_liveness_and_readiness() {
    let ready = serve_ops(health(true).await).await;
    let (status, media, cache, body) = get(ready, READINESS_PATH).await;
    assert_eq!(
        (status, media.as_str(), cache.as_str()),
        (200, "application/health+json", "no-store")
    );
    assert_eq!(body["status"], "pass");
    assert_eq!(
        body["checks"]["postgresql:responseTime"][0]["status"],
        "pass"
    );
    assert!(probe_http(ready, READINESS_PATH, TIMEOUT).await);

    let failing = serve_ops(health(false).await).await;
    let (status, _, _, body) = get(failing, READINESS_PATH).await;
    assert_eq!(status, 503);
    assert_eq!(body["status"], "fail");
    assert!(!probe_http(failing, READINESS_PATH, TIMEOUT).await);

    let (status, _, _, body) = get(failing, LIVENESS_PATH).await;
    assert_eq!(status, 200);
    assert_eq!(body["status"], "pass");
    assert!(body.get("checks").is_none());
    assert!(probe_http(failing, LIVENESS_PATH, TIMEOUT).await);

    let closed = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap();
    assert!(!probe_http(closed, READINESS_PATH, Duration::from_millis(200)).await);
}

fn descriptor() -> ServiceDescriptor {
    ServiceDescriptor::new(
        ServiceName::new("config-service").unwrap(),
        "0.1.0",
        Utc::now(),
    )
    .with_protocol(ProtocolRange::new(PLATFORM, 1, 3).unwrap())
    .with_capability(Capability::new("revision.plan", "1").unwrap())
}

async fn serve_grpc(health: HealthWatch, describe: Option<&ServiceDescriptor>) -> Channel {
    let (reporter, health_service) = tonic_health::server::health_reporter();
    tokio::spawn(publish_grpc_health(health, reporter, vec![String::new()]));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut router = Server::builder().add_service(health_service);
    if let Some(descriptor) = describe {
        router = router.add_service(ServiceInfoService::new(descriptor).into_server());
    }
    tokio::spawn(router.serve_with_incoming(TcpListenerStream::new(listener)));
    Channel::from_shared(format!("http://{address}"))
        .unwrap()
        .connect()
        .await
        .unwrap()
}

async fn serving(channel: Channel) -> ServingStatus {
    for _ in 0..100 {
        let status = HealthClient::new(channel.clone())
            .check(HealthCheckRequest {
                service: String::new(),
            })
            .await
            .unwrap()
            .into_inner()
            .status();
        if status != ServingStatus::Unknown {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    ServingStatus::Unknown
}

#[tokio::test]
async fn grpc_peers_describe_themselves_and_negotiate_revisions() {
    let expected = descriptor();
    let channel = serve_grpc(health(true).await, Some(&expected)).await;
    assert_eq!(serving(channel.clone()).await, ServingStatus::Serving);

    let described = describe_peer(channel.clone(), TIMEOUT).await.unwrap();
    assert_eq!(described, expected);

    let (_, negotiated) = negotiate_with_peer(
        channel.clone(),
        &ProtocolRange::up_to(PLATFORM, 2).unwrap(),
        TIMEOUT,
    )
    .await
    .unwrap();
    assert_eq!(negotiated.revision(), 2);

    let unknown = ProtocolRange::up_to("pingora.panel.gateway.v1", 1).unwrap();
    let error = negotiate_with_peer(channel, &unknown, TIMEOUT)
        .await
        .unwrap_err();
    assert_eq!(error.code.as_str(), ErrorCode::UNSUPPORTED_CAPABILITY);
}

#[tokio::test]
async fn peers_without_description_or_readiness_are_reported() {
    let channel = serve_grpc(health(false).await, None).await;
    assert_eq!(serving(channel.clone()).await, ServingStatus::NotServing);
    let error = describe_peer(channel, TIMEOUT).await.unwrap_err();
    assert_eq!(error.code.as_str(), ErrorCode::UNSUPPORTED_CAPABILITY);

    let starting = HealthWatch::fixed(HealthReport::starting(&ServiceIdentity::new("x", "0")));
    let channel = serve_grpc(starting, Some(&descriptor())).await;
    assert_eq!(serving(channel).await, ServingStatus::NotServing);
}
