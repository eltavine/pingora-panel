#![forbid(unsafe_code)]

//! Activates the benchmark configuration into a gateway state directory, so
//! `gatewayd` serves it as its last known good snapshot without a control
//! plane. Used by `panel/benchmarks/gateway`.
//!
//! ```text
//! bench_seed <state-dir> <listen-address> <upstream-address> [tls <secret-dir>]
//! ```
//!
//! Every host reaches one site whose single route proxies to the upstream.
//! With `tls` the listener terminates TLS with the `bench.crt` and
//! `bench.key` files of the secret directory the gateway is given.

use gateway_pingora::{AdapterOptions, DirectorySecrets};
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
    TlsProfile, UpstreamEndpoint, UpstreamPoolSpec,
};
use std::{
    collections::BTreeSet, net::SocketAddr, num::NonZeroU32, path::PathBuf, process::ExitCode,
    sync::Arc,
};

const USAGE: &str =
    "usage: bench_seed <state-dir> <listen-address> <upstream-address> [tls <secret-dir>]";

fn snapshot(listen: SocketAddr, upstream: SocketAddr, tls: bool) -> RuntimeSnapshot {
    let site = SiteId::new("bench").expect("valid site id");
    let pool = UpstreamPoolId::new("upstream").expect("valid pool id");
    let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
    let mut listener = ListenerRef::new("bench", listen.to_string());
    listener.default_site_id = Some(site.clone());
    if tls {
        listener.tls_profile_id = Some("bench".to_owned());
        snapshot.tls_profiles.push(TlsProfile {
            id: "bench".to_owned(),
            certificate_secret_id: "bench.crt".to_owned(),
            private_key_secret_id: "bench.key".to_owned(),
            min_protocol: "TLSv1.2".to_owned(),
            max_protocol: None,
            cipher_suites: Vec::new(),
            session_resumption: true,
            alpn: BTreeSet::new(),
        });
    }
    snapshot.listeners.push(listener);
    snapshot.sites.push(SiteSpec::new(
        site.clone(),
        "bench",
        vec![DomainSpec::new(
            NormalizedHost::new("bench.test").expect("valid host"),
        )],
    ));
    snapshot.upstream_pools.push(UpstreamPoolSpec::new(
        pool.clone(),
        "upstream",
        vec![UpstreamEndpoint::new(
            EndpointId::new("node").expect("valid endpoint id"),
            EndpointAddress::new(upstream.ip().to_string(), upstream.port(), false)
                .expect("valid endpoint address"),
        )],
    ));
    snapshot.routes.push(RouteSpec::new(
        RouteId::new("all").expect("valid route id"),
        site,
        1,
        RouteMatcher::PathPrefix {
            path: PathPrefix::new("/").expect("valid path prefix"),
        },
        RouteAction::Proxy {
            upstream_pool_id: pool,
        },
    ));
    snapshot.refresh_content_hash();
    snapshot
}

async fn seed(
    state: PathBuf,
    secrets: Option<PathBuf>,
    snapshot: RuntimeSnapshot,
) -> Result<(), String> {
    let mut adapter = AdapterOptions::default();
    if let Some(directory) = secrets {
        adapter = adapter.with_secrets(Arc::new(DirectorySecrets::new(directory)));
    }
    let runtime = build_gateway_runtime_with_options(
        state,
        Arc::new(ProcessRuntimeInfo::new("bench-seed", NonZeroU32::MIN)),
        GatewaydServiceOptions::default().with_adapter_options(adapter),
    )
    .await
    .map_err(|error| error.to_string())?;
    let engine = runtime.engine();
    let prepared = engine
        .prepare(PrepareRequest { snapshot })
        .await
        .map_err(|error| error.to_string())?;
    engine
        .activate(ActivateRequest {
            prepare_token: prepared.prepare_token,
            expected_active_hash: None,
        })
        .await
        .map_err(|error| error.to_string())?;
    runtime
        .background_tasks
        .shutdown_and_join_with_policy(BackgroundTaskShutdownPolicy::default())
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let (state, listen, upstream, secrets) = match arguments.as_slice() {
        [state, listen, upstream] => (state, listen, upstream, None),
        [state, listen, upstream, tls, secrets] if tls == "tls" => {
            (state, listen, upstream, Some(PathBuf::from(secrets)))
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let (Ok(listen), Ok(upstream)) = (listen.parse(), upstream.parse()) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let snapshot = snapshot(listen, upstream, secrets.is_some());
    match seed(PathBuf::from(state), secrets, snapshot).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bench_seed: {error}");
            ExitCode::FAILURE
        }
    }
}
