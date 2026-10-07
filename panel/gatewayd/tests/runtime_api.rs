#![cfg(unix)]
#![forbid(unsafe_code)]

//! Runtime operations on a real gateway process: state, reload, workers,
//! upstream health and drains, persisted across a restart, and shutdown.

use gateway_grpc_client::{GatewayGrpcClient, GatewayGrpcClientConfig};
use gatewayd::{
    DRAIN_TIMEOUT_MILLIS_ENV, GATEWAY_ADDRESS_ENV, OPS_ADDRESS_ENV, STATE_DIRECTORY_ENV,
    WORKER_COUNT_ENV,
};
use panel_application::{
    CachePurge, CommandContext, GatewayPort, GatewayRuntimePort, IdempotencyKey, RequestDeadline,
    RequestId, RequestScope,
};
use panel_domain::{
    EndpointAddress, EndpointId, NormalizedHost, PathPrefix, RevisionId, RouteId, SiteId,
    UpstreamPoolId,
};
use panel_ir::{
    DomainSpec, ListenerRef, RouteAction, RouteMatcher, RouteSpec, RuntimeSnapshot, SiteSpec,
    UpstreamEndpoint, UpstreamPoolSpec,
};
use std::{
    net::{SocketAddr, TcpListener},
    path::Path,
    process::{Child, Command},
    time::{Duration, Instant},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Free loopback addresses, held together until all are known so that no
/// two are the same.
fn reserve<const N: usize>() -> [SocketAddr; N] {
    let held: [TcpListener; N] = std::array::from_fn(|_| TcpListener::bind("127.0.0.1:0").unwrap());
    held.map(|listener| listener.local_addr().unwrap())
}

struct Gateway(Child);

impl Gateway {
    fn spawn(address: SocketAddr, ops: SocketAddr, state: &Path) -> Self {
        Self(
            Command::new(env!("CARGO_BIN_EXE_gatewayd"))
                .env(GATEWAY_ADDRESS_ENV, address.to_string())
                .env(OPS_ADDRESS_ENV, ops.to_string())
                .env(STATE_DIRECTORY_ENV, state)
                .env(WORKER_COUNT_ENV, "2")
                .env(DRAIN_TIMEOUT_MILLIS_ENV, "300")
                .spawn()
                .unwrap(),
        )
    }

    async fn exit_code(&mut self) -> Option<i32> {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                return status.code();
            }
            assert!(Instant::now() < deadline, "gatewayd did not exit");
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn command(key: &str) -> CommandContext {
    CommandContext::new(
        RequestId::new(format!("request-{key}")).unwrap(),
        RequestId::new("runtime-flow").unwrap(),
        "operator",
        RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
        IdempotencyKey::new(key).unwrap(),
    )
    .unwrap()
}

fn scope() -> RequestScope {
    RequestScope::new(RequestId::new("runtime-read").unwrap())
}

fn snapshot(listen: SocketAddr) -> RuntimeSnapshot {
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    snapshot
        .listeners
        .push(ListenerRef::new("http", listen.to_string()));
    snapshot.sites.push(SiteSpec::new(
        SiteId::new("site").unwrap(),
        "site",
        vec![DomainSpec::new(NormalizedHost::new("example.com").unwrap())],
    ));
    snapshot.upstream_pools.push(UpstreamPoolSpec::new(
        UpstreamPoolId::new("app").unwrap(),
        "app",
        (0..2)
            .map(|index| {
                UpstreamEndpoint::new(
                    EndpointId::new(format!("node-{index}")).unwrap(),
                    EndpointAddress::new("127.0.0.1", 9000 + index, false).unwrap(),
                )
            })
            .collect(),
    ));
    snapshot.routes.push(RouteSpec::new(
        RouteId::new("app").unwrap(),
        SiteId::new("site").unwrap(),
        1,
        RouteMatcher::PathPrefix {
            path: PathPrefix::new("/").unwrap(),
        },
        RouteAction::Proxy {
            upstream_pool_id: UpstreamPoolId::new("app").unwrap(),
        },
    ));
    snapshot.refresh_content_hash();
    snapshot
}

/// What the gateway's operational listener answers at `path`.
async fn ops_get(ops: SocketAddr, path: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect(ops).await.unwrap();
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).await.unwrap();
    response
}

async fn connect(address: SocketAddr) -> GatewayGrpcClient {
    let client = GatewayGrpcClient::connect_lazy(
        format!("http://{address}"),
        GatewayGrpcClientConfig::default(),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    while client.data_plane(scope()).await.is_err() {
        assert!(Instant::now() < deadline, "gatewayd never answered");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    client
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runtime_operations_persist_and_shut_the_gateway_down() {
    let state = tempfile::tempdir().unwrap();
    let [management, listen, ops] = reserve();
    let mut gateway = Gateway::spawn(management, ops, state.path());
    let client = connect(management).await;

    let prepared = client
        .prepare_with_context(command("prepare"), snapshot(listen))
        .await
        .unwrap();
    client
        .activate_with_context(
            command("activate"),
            prepared.prepare_token().to_owned(),
            None,
        )
        .await
        .unwrap();
    let mut plane = client.data_plane(scope()).await.unwrap();
    for _ in 0..100 {
        if !plane.listeners.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
        plane = client.data_plane(scope()).await.unwrap();
    }
    assert_eq!(plane.listeners[0].address, listen.to_string());
    assert_eq!(plane.worker_count, 2);
    assert_eq!(plane.active_revision_id, Some(1));
    assert_eq!(plane.engine_version, "0.9.0");
    let ready = ops_get(ops, "/readyz").await;
    assert!(ready.starts_with("HTTP/1.1 200"), "{ready}");
    let metrics = ops_get(ops, "/metrics").await;
    assert!(metrics.starts_with("HTTP/1.1 200"), "{metrics}");
    assert!(
        metrics.contains("pingora_panel_gateway_config_revision 1\n"),
        "{metrics}"
    );

    let reloaded = client.reload(command("reload")).await.unwrap();
    assert_eq!(reloaded.generation, plane.generation + 1);
    let resized = client
        .set_worker_count(command("workers"), 1)
        .await
        .unwrap();
    assert_eq!(resized.worker_count, 1);
    assert!(client
        .set_worker_count(command("too-many"), 0)
        .await
        .is_err());

    let health = client.upstream_health(scope()).await.unwrap();
    assert_eq!(health.upstreams[0].endpoints.len(), 2);
    assert_eq!(health.active_revision_id, Some(1));
    let drained = client
        .set_endpoint_drained(command("drain"), "app".into(), "node-0".into(), true)
        .await
        .unwrap();
    assert!(drained.endpoints[0].drained);
    assert!(client
        .set_endpoint_drained(command("missing"), "app".into(), "ghost".into(), true)
        .await
        .is_err());
    let files = client.file_checks(scope()).await.unwrap();
    assert_eq!(files.active_revision_id, Some(1));
    assert!(files.checked_at.is_some());
    assert!(files.private_keys.is_empty() && files.static_roots.is_empty());
    let cache = client.cache_stats(scope()).await.unwrap();
    assert_eq!(cache.max_bytes, 256 << 20);
    assert_eq!((cache.bytes, cache.entries), (0, 0));
    assert!(cache.since.is_some() && cache.observed_at.is_some());
    let purged = client
        .purge_cache(command("purge"), CachePurge::All)
        .await
        .unwrap();
    assert_eq!(purged.keys, 0);
    assert!(client
        .purge_cache(
            command("purge-nowhere"),
            CachePurge::Urls(vec!["https://nowhere.test/".into()])
        )
        .await
        .is_err());

    client.shutdown(command("shutdown")).await.unwrap();
    assert_eq!(gateway.exit_code().await, Some(0));

    let mut gateway = Gateway::spawn(management, ops, state.path());
    let client = connect(management).await;
    let mut plane = client.data_plane(scope()).await.unwrap();
    for _ in 0..100 {
        if !plane.listeners.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
        plane = client.data_plane(scope()).await.unwrap();
    }
    assert_eq!(plane.worker_count, 1, "the worker count survives a restart");
    let health = client.upstream_health(scope()).await.unwrap();
    assert!(
        health.upstreams[0].endpoints[0].drained,
        "drains survive a restart"
    );
    assert!(!health.upstreams[0].endpoints[1].drained);
    client.shutdown(command("shutdown-again")).await.unwrap();
    assert_eq!(gateway.exit_code().await, Some(0));
}
