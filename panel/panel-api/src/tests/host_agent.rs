use super::*;
use panel_application::{
    AgentCapability, AgentDescription, CapabilityState, CapabilityStatus, DirectoriesReport,
    DirectoryKind, DirectoryUsage, HostAgentPort, ListenersReport, ListeningProcess, PortListener,
    RequestScope,
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
