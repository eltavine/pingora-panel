#![forbid(unsafe_code)]

use ops_grpc_client::OpsAgentClient;
use panel_application::{AgentCapability, DirectoryKind, HostAgentPort, RequestId, RequestScope};
use panel_contracts::{
    common::v1 as common,
    ops::v1::{
        self as wire,
        agent_server::{Agent, AgentServer},
        directories_server::{Directories, DirectoriesServer},
    },
};
use tokio_stream::wrappers::TcpListenerStream;
use tonic::{
    transport::{Channel, Server},
    Request, Response, Status,
};

struct FakeAgent;

#[tonic::async_trait]
impl Agent for FakeAgent {
    async fn describe(
        &self,
        request: Request<wire::AgentDescribeRequest>,
    ) -> Result<Response<wire::AgentDescribeResponse>, Status> {
        assert_eq!(
            request.into_inner().context.unwrap().request_id,
            "request-1"
        );
        Ok(Response::new(wire::AgentDescribeResponse {
            description: Some(wire::AgentDescription {
                version: Some(common::Version {
                    build: "1.2.3".into(),
                    protocol: "pingora.panel.ops.v1@1..=1".into(),
                    ..common::Version::default()
                }),
                hostname: "edge-1".into(),
                capabilities: vec![wire::AgentCapability {
                    capability: wire::Capability::Directories.into(),
                    state: wire::CapabilityState::Available.into(),
                    detail: String::new(),
                }],
            }),
            error: None,
        }))
    }
}

struct FakeDirectories {
    fail: bool,
}

#[tonic::async_trait]
impl Directories for FakeDirectories {
    async fn usage(
        &self,
        _: Request<wire::DirectoriesUsageRequest>,
    ) -> Result<Response<wire::DirectoriesUsageResponse>, Status> {
        Ok(Response::new(if self.fail {
            wire::DirectoriesUsageResponse {
                error: Some(common::Error {
                    code: "STORAGE_UNAVAILABLE".into(),
                    message: "the disk is gone".into(),
                    retryable: true,
                    diagnostics: Vec::new(),
                }),
                ..wire::DirectoriesUsageResponse::default()
            }
        } else {
            wire::DirectoriesUsageResponse {
                observed_at: Some(std::time::SystemTime::now().into()),
                directories: vec![wire::DirectoryUsage {
                    kind: wire::DirectoryKind::Certificates.into(),
                    path: "/etc/pingora-panel/certificates".into(),
                    present: true,
                    bytes: 4096,
                    files: 4,
                    unreadable: 1,
                    truncated: false,
                }],
                error: None,
            }
        }))
    }
}

async fn client(fail: bool) -> OpsAgentClient {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(
        Server::builder()
            .add_service(AgentServer::new(FakeAgent))
            .add_service(DirectoriesServer::new(FakeDirectories { fail }))
            .serve_with_incoming(TcpListenerStream::new(listener)),
    );
    OpsAgentClient::from_channel(
        Channel::from_shared(format!("http://{address}"))
            .unwrap()
            .connect_lazy(),
    )
}

fn scope() -> RequestScope {
    RequestScope::new(RequestId::new("request-1").unwrap())
}

#[tokio::test]
async fn the_agent_and_its_directories_reach_the_application() {
    let client = client(false).await;
    let description = client.agent(scope()).await.unwrap();
    assert_eq!(description.build, "1.2.3");
    assert_eq!(description.hostname, "edge-1");
    assert!(description.has(AgentCapability::Directories));

    let report = client.directories(scope()).await.unwrap();
    assert!(report.observed_at.is_some());
    let certificates = &report.directories[0];
    assert_eq!(certificates.kind, DirectoryKind::Certificates);
    assert_eq!(
        (
            certificates.bytes,
            certificates.files,
            certificates.unreadable
        ),
        (4096, 4, 1)
    );
}

#[tokio::test]
async fn the_agents_errors_keep_their_codes() {
    let refused = client(true).await.directories(scope()).await.unwrap_err();
    assert_eq!(refused.code.as_str(), "STORAGE_UNAVAILABLE");
    assert!(refused.retryable);
}
