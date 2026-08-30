#![cfg(unix)]

use gatewayd::{
    DRAIN_TIMEOUT_MILLIS_ENV, GATEWAY_ADDRESS_ENV, STATE_DIRECTORY_ENV, WORKER_COUNT_ENV,
};
use panel_contracts::{
    common::v1 as common,
    gateway::v1::{gateway_engine_client::GatewayEngineClient, StatusRequest},
};
use std::{
    fs,
    net::{SocketAddr, TcpListener},
    path::PathBuf,
    process::{Child, Command, ExitStatus},
    time::{Duration, Instant},
};
use tokio::time::sleep;
use tonic::transport::Channel;
use tonic_health::pb::{
    health_check_response::ServingStatus, health_client::HealthClient, HealthCheckRequest,
};
use uuid::Uuid;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);

struct TemporaryDirectory(PathBuf);

impl TemporaryDirectory {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!("pingora-panel-process-{}", Uuid::new_v4())))
    }
}

impl Drop for TemporaryDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct GatewayProcess {
    child: Child,
}

impl GatewayProcess {
    fn spawn(address: SocketAddr, state_directory: &PathBuf) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_gatewayd"))
            .env(GATEWAY_ADDRESS_ENV, address.to_string())
            .env(STATE_DIRECTORY_ENV, state_directory)
            .env(WORKER_COUNT_ENV, "2")
            .env(DRAIN_TIMEOUT_MILLIS_ENV, "300")
            .spawn()
            .unwrap();
        Self { child }
    }

    fn terminate(&mut self) {
        // SAFETY: the child ID comes from a live process owned by this guard, and
        // SIGTERM does not dereference memory in either process.
        let result = unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM) };
        assert_eq!(result, 0, "failed to send SIGTERM to gatewayd");
    }

    async fn wait_for_exit(&mut self) -> ExitStatus {
        let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "gatewayd did not exit after SIGTERM"
            );
            sleep(Duration::from_millis(25)).await;
        }
    }
}

impl Drop for GatewayProcess {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn reserve_address() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap()
}

async fn wait_until_serving(address: SocketAddr) -> Channel {
    let deadline = Instant::now() + STARTUP_TIMEOUT;
    loop {
        if let Ok(channel) = Channel::from_shared(format!("http://{address}"))
            .unwrap()
            .connect()
            .await
        {
            let mut health = HealthClient::new(channel.clone());
            if let Ok(response) = health
                .check(HealthCheckRequest {
                    service: String::new(),
                })
                .await
            {
                if response.into_inner().status == ServingStatus::Serving as i32 {
                    return channel;
                }
            }
        }
        assert!(Instant::now() < deadline, "gatewayd did not become ready");
        sleep(Duration::from_millis(25)).await;
    }
}

async fn wait_until_not_serving(health: &mut HealthClient<Channel>) {
    let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
    loop {
        if let Ok(response) = health
            .check(HealthCheckRequest {
                service: String::new(),
            })
            .await
        {
            if response.into_inner().status == ServingStatus::NotServing as i32 {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "gatewayd did not withdraw readiness before shutdown"
        );
        sleep(Duration::from_millis(10)).await;
    }
}

fn context(request_id: &str) -> common::RequestContext {
    common::RequestContext {
        request_id: request_id.into(),
        correlation_id: "process-lifecycle".into(),
        actor: "process-test".into(),
        deadline: String::new(),
        idempotency_key: String::new(),
        schema_version: panel_contracts::PROTOCOL_VERSION.into(),
    }
}

#[tokio::test]
async fn sigterm_exits_cleanly_and_the_same_address_can_restart() {
    let state = TemporaryDirectory::new();
    let address = reserve_address();
    let mut first = GatewayProcess::spawn(address, &state.0);
    let channel = wait_until_serving(address).await;
    let mut client = GatewayEngineClient::new(channel.clone());
    let mut health = HealthClient::new(channel.clone());

    let status = client
        .status(StatusRequest {
            context: Some(context("process-status")),
        })
        .await
        .unwrap()
        .into_inner();
    let runtime = status.runtime.unwrap();
    assert_eq!(runtime.gateway_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(runtime.data_plane_version, "0.8.0");
    assert_eq!(runtime.worker_count, 2);
    assert_ne!(runtime.started_at_unix_seconds, 0);

    first.terminate();
    wait_until_not_serving(&mut health).await;
    drop(health);
    drop(client);
    drop(channel);
    assert!(first.wait_for_exit().await.success());

    let mut restarted = GatewayProcess::spawn(address, &state.0);
    let restarted_channel = wait_until_serving(address).await;
    drop(restarted_channel);
    restarted.terminate();
    assert!(restarted.wait_for_exit().await.success());
}
