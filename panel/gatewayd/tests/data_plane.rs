#![forbid(unsafe_code)]

//! The composed gateway serves what it activates and, after a restart, keeps
//! serving its last known good snapshot without any control plane.

use gateway_pingora::DataPlaneOptions;
use gatewayd::{
    build_gateway_runtime_with_options, BackgroundTaskShutdownPolicy, GatewaydServiceOptions,
    ProcessRuntimeInfo,
};
use panel_domain::{
    EndpointAddress, EndpointId, NormalizedHost, PathPrefix, RevisionId, RouteId, SiteId,
    UpstreamPoolId,
};
use panel_engine::{ActivateRequest, PrepareRequest};
use panel_ir::{
    DomainSpec, ListenerRef, RouteAction, RouteMatcher, RouteSpec, RuntimeSnapshot, SiteSpec,
    UpstreamEndpoint, UpstreamPoolSpec,
};
use std::{
    net::SocketAddr,
    num::{NonZeroU32, NonZeroUsize},
    path::Path,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};

async fn upstream() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buffer = [0; 4096];
                let _ = stream.read(&mut buffer).await;
                let _ = stream
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 8\r\nconnection: close\r\n\r\nupstream")
                    .await;
            });
        }
    });
    address
}

async fn get(address: SocketAddr) -> Option<String> {
    let mut stream = TcpStream::connect(address).await.ok()?;
    stream
        .write_all(b"GET / HTTP/1.1\r\nhost: example.com\r\nconnection: close\r\n\r\n")
        .await
        .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).await.ok()?;
    Some(response)
}

async fn eventually_served(address: SocketAddr) -> String {
    for _ in 0..200 {
        if let Some(response) = get(address).await {
            return response;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("{address} was never served");
}

fn snapshot(listen: SocketAddr, upstream: SocketAddr) -> RuntimeSnapshot {
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
        vec![UpstreamEndpoint::new(
            EndpointId::new("node").unwrap(),
            EndpointAddress::new("127.0.0.1", upstream.port(), false).unwrap(),
        )],
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

/// Runs a gateway from `state` until the returned sender fires.
async fn gateway(
    state: &Path,
    activate: Option<RuntimeSnapshot>,
) -> (oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
    let runtime = build_gateway_runtime_with_options(
        state.to_path_buf(),
        Arc::new(ProcessRuntimeInfo::new("test", NonZeroU32::MIN)),
        GatewaydServiceOptions::default(),
    )
    .await
    .unwrap();
    if let Some(snapshot) = activate {
        let engine = runtime.engine();
        let prepared = engine.prepare(PrepareRequest { snapshot }).await.unwrap();
        engine
            .activate(ActivateRequest {
                prepare_token: prepared.prepare_token,
                expected_active_hash: None,
            })
            .await
            .unwrap();
    }
    let plane = runtime.data_plane(
        DataPlaneOptions::new(NonZeroUsize::MIN).with_drain_timeout(Duration::from_secs(1)),
    );
    let (stop, stopped) = oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        plane
            .run(async {
                let _ = stopped.await;
            })
            .await;
        let _ = runtime
            .background_tasks
            .shutdown_and_join_with_policy(BackgroundTaskShutdownPolicy::default())
            .await;
    });
    (stop, task)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn activated_snapshots_are_served_and_survive_a_restart() {
    let state = tempfile::tempdir().unwrap();
    let upstream = upstream().await;
    let listen = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();

    let (stop, task) = gateway(state.path(), Some(snapshot(listen, upstream))).await;
    let response = eventually_served(listen).await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.ends_with("upstream"), "{response}");
    stop.send(()).unwrap();
    task.await.unwrap();
    assert!(get(listen).await.is_none());

    let (stop, task) = gateway(state.path(), None).await;
    let response = eventually_served(listen).await;
    assert!(response.ends_with("upstream"), "{response}");
    stop.send(()).unwrap();
    task.await.unwrap();
}
