#![forbid(unsafe_code)]

use panel_control_runtime::{
    ControlPlane, DefaultAddresses, DATA_DIR_ENV, HEALTH_INTERVAL_MS_ENV, NATS_URL_ENV,
};
use panel_health::HealthStatus;
use panel_service::Environment;
use std::{collections::HashMap, ffi::OsString, net::SocketAddr, time::Duration};

fn any_port() -> DefaultAddresses {
    DefaultAddresses {
        ops: "127.0.0.1:0".parse().unwrap(),
        grpc: "127.0.0.1:0".parse().unwrap(),
    }
}

fn free_port() -> SocketAddr {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

#[tokio::test]
async fn the_api_reaches_every_module_of_its_process() {
    let data = tempfile::tempdir().unwrap();
    let values: HashMap<String, OsString> = [
        (DATA_DIR_ENV, data.path().display().to_string()),
        (NATS_URL_ENV, "nats://127.0.0.1:1".to_owned()),
        (HEALTH_INTERVAL_MS_ENV, "50".to_owned()),
        (panel_api_server::HTTP_ADDRESS_ENV, free_port().to_string()),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), OsString::from(value)))
    .collect();
    let mut env = Environment::from_lookup(move |name| values.get(name).cloned());
    let modules = panel_control::modules().map(|module| module.with_addresses(any_port()));

    let control_plane = ControlPlane::start(&mut env, &modules).await.unwrap();
    for module in control_plane.modules() {
        assert_eq!(module.grpc_address(), None);
    }
    let api = control_plane.modules().last().unwrap();
    assert_eq!(
        api.descriptor().service().as_str(),
        panel_api_server::SERVICE
    );
    let mut health = api.health();
    let peers = [
        config_service::SERVICE,
        audit_service::SERVICE,
        automation_service::SERVICE,
        observability_service::SERVICE,
    ];
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let report = health.current();
            let reached = peers.iter().all(|peer| {
                report.checks().iter().any(|(name, components)| {
                    name.starts_with(peer)
                        && components
                            .iter()
                            .all(|component| component.status() == HealthStatus::Pass)
                })
            });
            if reached {
                break;
            }
            assert!(health.changed().await);
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!(
            "the API reaches every module in process: {:?}",
            health.current().checks()
        )
    });

    tokio::time::timeout(Duration::from_secs(5), control_plane.stop())
        .await
        .expect("the modules stop");
}
