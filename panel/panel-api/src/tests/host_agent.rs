use super::*;
use panel_application::{
    AgentCapability, AgentDescription, CapabilityState, CapabilityStatus, CommandContext,
    ContainerState, ContainerSummary, DirectoriesReport, DirectoryKind, DirectoryUsage,
    GatewayContainer, GatewayServiceAction, GatewayServiceStatus, HostAgentPort, ListenersReport,
    ListeningProcess, PortListener, RequestScope,
};
use serde_json::Value;
use std::time::{Duration, UNIX_EPOCH};

struct Agent {
    reachable: bool,
}

#[async_trait]
impl HostAgentPort for Agent {
    async fn agent(&self, _scope: RequestScope) -> Result<AgentDescription> {
        if !self.reachable {
            return Err(PanelError::unavailable(
                "transport error: /run/pingora-panel-ops/agent.sock: connection refused",
            ));
        }
        Ok(AgentDescription {
            build: "0.1.0".into(),
            protocol: "pingora.panel.ops.v1@1..=1".into(),
            hostname: "web-1".into(),
            capabilities: vec![
                CapabilityStatus {
                    capability: AgentCapability::Directories,
                    state: CapabilityState::Available,
                    detail: String::new(),
                },
                CapabilityStatus {
                    capability: AgentCapability::Listeners,
                    state: CapabilityState::Denied,
                    detail: "grant CAP_DAC_READ_SEARCH".into(),
                },
            ],
        })
    }

    async fn directories(&self, _scope: RequestScope) -> Result<DirectoriesReport> {
        Ok(DirectoriesReport {
            observed_at: Some(UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
            directories: vec![DirectoryUsage {
                kind: DirectoryKind::Logs,
                path: "/var/log/pingora-panel".into(),
                present: true,
                bytes: 1_048_576,
                files: 12,
                unreadable: 0,
                truncated: false,
            }],
        })
    }

    async fn gateway_service(&self, _scope: RequestScope) -> Result<GatewayServiceStatus> {
        Ok(GatewayServiceStatus {
            observed_at: Some(UNIX_EPOCH + Duration::from_secs(1_800_000_060)),
            container: GatewayContainer {
                engine: "docker".into(),
                container: ContainerSummary {
                    id: "g7".into(),
                    names: vec!["pingora-panel-gatewayd-1".into()],
                    image: "localhost/pingora-panel:dev".into(),
                    image_id: "sha256:cc".into(),
                    created: None,
                    state: ContainerState::Running,
                    status: "Up 1 minute (healthy)".into(),
                    ports: Vec::new(),
                    labels: Default::default(),
                    compose_project: Some("pingora-panel".into()),
                    addresses: Vec::new(),
                },
                started_at: Some(UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
                finished_at: None,
                exit_code: 0,
                restarts: 0,
                health: "healthy".into(),
            },
        })
    }

    async fn change_gateway_service(
        &self,
        context: CommandContext,
        action: GatewayServiceAction,
    ) -> Result<GatewayServiceStatus> {
        if action == GatewayServiceAction::Stop && context.actor() == "careless" {
            return Err(PanelError::precondition_failed(
                "the docker engine is disabled",
            ));
        }
        let mut status = self.gateway_service(context.scope()).await?;
        if action == GatewayServiceAction::Stop {
            status.container.container.state = ContainerState::Exited;
            status.container.health = String::new();
            status.container.finished_at = Some(UNIX_EPOCH + Duration::from_secs(1_800_000_120));
            status.container.exit_code = 0;
        }
        Ok(status)
    }

    async fn listeners(&self, _scope: RequestScope, ports: Vec<u16>) -> Result<ListenersReport> {
        Ok(ListenersReport {
            observed_at: Some(UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
            listeners: ports
                .into_iter()
                .map(|port| PortListener {
                    address: "0.0.0.0".into(),
                    port,
                    uid: 0,
                    processes: vec![ListeningProcess {
                        pid: 812,
                        name: "nginx".into(),
                        executable: "/usr/sbin/nginx".into(),
                        uid: 33,
                    }],
                })
                .collect(),
        })
    }
}

fn app(agent: Option<Agent>) -> axum::Router {
    let state = ApiState::new(Arc::new(GatewayService::new(
        Arc::new(FakeGateway),
        Arc::new(IdentityCompiler),
    )));
    router(match agent {
        Some(agent) => state.with_host_agent(Arc::new(agent)),
        None => state,
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
async fn an_installation_without_the_agent_says_so() {
    let app = app(None);
    let (status, agent) = get(&app, "/api/v1/host/agent").await;
    assert_eq!(status, StatusCode::OK, "{agent}");
    assert_eq!(agent["status"], "not_configured");
    assert_eq!(agent["capabilities"], Value::Array(Vec::new()));

    let (status, problem) = get(&app, "/api/v1/host/directories").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
    assert_eq!(problem["code"], "UNSUPPORTED_CAPABILITY");
}

#[tokio::test]
async fn the_agents_capabilities_and_directories_are_served() {
    let app = app(Some(Agent { reachable: true }));
    let (status, agent) = get(&app, "/api/v1/host/agent").await;
    assert_eq!(status, StatusCode::OK, "{agent}");
    assert_eq!(agent["status"], "connected");
    assert_eq!(agent["build"], "0.1.0");
    assert_eq!(agent["hostname"], "web-1");
    assert_eq!(agent["capabilities"][0]["capability"], "directories");
    assert_eq!(agent["capabilities"][0]["state"], "available");
    assert_eq!(agent["capabilities"][1]["capability"], "listeners");
    assert_eq!(agent["capabilities"][1]["state"], "denied");
    assert_eq!(
        agent["capabilities"][1]["detail"],
        "grant CAP_DAC_READ_SEARCH"
    );

    let (status, directories) = get(&app, "/api/v1/host/directories").await;
    assert_eq!(status, StatusCode::OK, "{directories}");
    assert_eq!(directories["observed_at"], "2027-01-15T08:00:00Z");
    let logs = &directories["directories"][0];
    assert_eq!(logs["kind"], "logs");
    assert_eq!(logs["bytes"], 1_048_576);
    assert_eq!(logs["files"], 12);
}

#[tokio::test]
async fn an_unreachable_agent_is_a_state_not_a_leak() {
    let (status, agent) = get(&app(Some(Agent { reachable: false })), "/api/v1/host/agent").await;
    assert_eq!(status, StatusCode::OK, "{agent}");
    assert_eq!(agent["status"], "unreachable");
    assert!(
        !agent.to_string().contains("agent.sock"),
        "the transport's error stays in the log"
    );
}

#[tokio::test]
async fn what_holds_ports_is_served_for_the_ports_asked_for() {
    let app = app(Some(Agent { reachable: true }));
    let (status, found) = get(&app, "/api/v1/host/listeners").await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert_eq!(found["listeners"].as_array().map(Vec::len), Some(0));

    let (status, found) = get(&app, "/api/v1/host/listeners?ports=443,8443").await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert_eq!(found["observed_at"], "2027-01-15T08:00:00Z");
    assert_eq!(found["listeners"][0]["port"], 443);
    assert_eq!(found["listeners"][1]["port"], 8443);
    assert_eq!(found["listeners"][0]["processes"][0]["name"], "nginx");
    assert_eq!(
        found["listeners"][0]["processes"][0]["executable"],
        "/usr/sbin/nginx"
    );

    for refused in [
        "ports=0",
        "ports=65536",
        "ports=http",
        "ports=1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17",
    ] {
        let (status, problem) = get(&app, &format!("/api/v1/host/listeners?{refused}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}: {problem}");
    }
}

async fn post(app: &axum::Router, path: &str, actor: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::post(path)
                .header("x-actor", actor)
                .header("idempotency-key", "gateway-1")
                .header("x-deadline", "2099-01-01T00:00:00Z")
                .body(Body::empty())
                .unwrap(),
        )
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
async fn the_gateway_service_is_read_and_changed() {
    let router = app(Some(Agent { reachable: true }));
    let (status, service) = get(&router, "/api/v1/host/gateway-service").await;
    assert_eq!(status, StatusCode::OK, "{service}");
    assert_eq!(service["supervisor"], "container");
    assert_eq!(service["observed_at"], "2027-01-15T08:01:00Z");
    let gateway = &service["container"];
    assert_eq!(gateway["engine"], "docker");
    assert_eq!(gateway["name"], "pingora-panel-gatewayd-1");
    assert_eq!(gateway["state"], "running");
    assert_eq!(gateway["health"], "healthy");
    assert_eq!(gateway["started_at"], "2027-01-15T08:00:00Z");
    assert_eq!(gateway["finished_at"], Value::Null);
    assert_eq!(gateway["exit_code"], Value::Null);

    let (status, service) = post(&router, "/api/v1/host/gateway-service/stop", "ops").await;
    assert_eq!(status, StatusCode::OK, "{service}");
    let gateway = &service["container"];
    assert_eq!(gateway["state"], "exited");
    assert_eq!(gateway["health"], Value::Null);
    assert_eq!(gateway["finished_at"], "2027-01-15T08:02:00Z");
    assert_eq!(gateway["exit_code"], 0);

    let (status, problem) = post(&router, "/api/v1/host/gateway-service/stop", "careless").await;
    assert_eq!(status, StatusCode::PRECONDITION_FAILED, "{problem}");

    let (status, problem) = post(&router, "/api/v1/host/gateway-service/reboot", "ops").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");

    let (status, problem) = post(&app(None), "/api/v1/host/gateway-service/start", "ops").await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{problem}");
}
