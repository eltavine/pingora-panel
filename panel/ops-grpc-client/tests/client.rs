#![forbid(unsafe_code)]

use ops_grpc_client::OpsAgentClient;
use panel_application::{
    AgentCapability, CommandContext, ContainerAction, ContainerLogQuery, ContainerLogStart,
    ContainerLogStream, ContainerNetworkStats, ContainerState, ContainersPort, DirectoryKind,
    GatewayServiceAction, HostAgentPort, IdempotencyKey, ImagesPort, RequestDeadline, RequestId,
    RequestScope,
};
use panel_contracts::{
    common::v1 as common,
    ops::v1::{
        self as wire,
        agent_server::{Agent, AgentServer},
        containers_server::{Containers, ContainersServer},
        directories_server::{Directories, DirectoriesServer},
        gateway_service_server::{GatewayService, GatewayServiceServer},
        gateway_service_status::Supervisor,
        images_server::{Images, ImagesServer},
        listeners_server::{Listeners, ListenersServer},
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

struct FakeListeners;

#[tonic::async_trait]
impl Listeners for FakeListeners {
    async fn list(
        &self,
        request: Request<wire::ListenersListRequest>,
    ) -> Result<Response<wire::ListenersListResponse>, Status> {
        assert_eq!(request.into_inner().ports, vec![80, 443]);
        Ok(Response::new(wire::ListenersListResponse {
            observed_at: Some(std::time::SystemTime::now().into()),
            listeners: vec![wire::Listener {
                address: "0.0.0.0".into(),
                port: 443,
                uid: 0,
                processes: vec![wire::ListeningProcess {
                    pid: 812,
                    name: "nginx".into(),
                    executable: "/usr/sbin/nginx".into(),
                    uid: 33,
                }],
            }],
            error: None,
        }))
    }
}

/// Refuses to stop the gateway, as a disabled engine would.
struct FakeGateway;

fn gateway(state: wire::ContainerState, restarts: u32) -> wire::GatewayServiceStatus {
    wire::GatewayServiceStatus {
        observed_at: Some(std::time::SystemTime::now().into()),
        supervisor: Some(Supervisor::Container(wire::GatewayContainer {
            engine: "docker".into(),
            container: Some(wire::Container {
                id: "g7".into(),
                names: vec!["pingora-panel-gatewayd-1".into()],
                state: state.into(),
                compose_project: "pingora-panel".into(),
                ..wire::Container::default()
            }),
            restarts,
            health: "healthy".into(),
            ..wire::GatewayContainer::default()
        })),
    }
}

#[tonic::async_trait]
impl GatewayService for FakeGateway {
    async fn status(
        &self,
        _: Request<wire::GatewayServiceStatusRequest>,
    ) -> Result<Response<wire::GatewayServiceStatusResponse>, Status> {
        Ok(Response::new(wire::GatewayServiceStatusResponse {
            status: Some(gateway(wire::ContainerState::Running, 0)),
            error: None,
        }))
    }

    async fn change(
        &self,
        request: Request<wire::GatewayServiceChangeRequest>,
    ) -> Result<Response<wire::GatewayServiceChangeResponse>, Status> {
        let request = request.into_inner();
        assert_eq!(request.context.as_ref().unwrap().actor, "ops");
        Ok(Response::new(match request.action() {
            wire::GatewayServiceAction::Stop => wire::GatewayServiceChangeResponse {
                status: None,
                error: Some(common::Error {
                    code: "PRECONDITION_FAILED".into(),
                    message: "the docker engine is disabled".into(),
                    retryable: false,
                    diagnostics: Vec::new(),
                }),
            },
            action => wire::GatewayServiceChangeResponse {
                status: Some(gateway(
                    wire::ContainerState::Running,
                    u32::from(action == wire::GatewayServiceAction::Restart),
                )),
                error: None,
            },
        }))
    }
}

/// nginx, which a running container uses and so cannot be removed.
struct FakeImages;

#[tonic::async_trait]
impl Images for FakeImages {
    async fn list(
        &self,
        request: Request<wire::ImagesListRequest>,
    ) -> Result<Response<wire::ImagesListResponse>, Status> {
        assert_eq!(request.into_inner().search, "nginx");
        Ok(Response::new(wire::ImagesListResponse {
            observed_at: Some(at(10).into()),
            images: vec![wire::Image {
                id: "sha256:aa".into(),
                tags: vec!["nginx:1.27".into()],
                created: Some(at(0).into()),
                size_bytes: 50_000_000,
                containers: 1,
                labels: [("maintainer".to_owned(), "NGINX".to_owned())].into(),
                ..wire::Image::default()
            }],
            error: None,
        }))
    }

    async fn inspect(
        &self,
        _: Request<wire::ImagesInspectRequest>,
    ) -> Result<Response<wire::ImagesInspectResponse>, Status> {
        Ok(Response::new(wire::ImagesInspectResponse {
            detail: Some(wire::ImageDetail {
                image: Some(wire::Image {
                    id: "sha256:aa".into(),
                    ..wire::Image::default()
                }),
                architecture: "arm64".into(),
                exposed_ports: vec!["80/tcp".into()],
                layers: 7,
                ..wire::ImageDetail::default()
            }),
            error: None,
        }))
    }

    async fn remove(
        &self,
        request: Request<wire::ImagesRemoveRequest>,
    ) -> Result<Response<wire::ImagesRemoveResponse>, Status> {
        assert!(request.into_inner().force);
        Ok(Response::new(wire::ImagesRemoveResponse {
            error: Some(common::Error {
                code: "CONFLICT".into(),
                message: "image is being used by running container b2".into(),
                retryable: false,
                diagnostics: Vec::new(),
            }),
            ..wire::ImagesRemoveResponse::default()
        }))
    }
}

/// One container to inspect; every action on it is refused.
struct FakeContainers;

#[tonic::async_trait]
impl Containers for FakeContainers {
    async fn engines(
        &self,
        _: Request<wire::ContainersEnginesRequest>,
    ) -> Result<Response<wire::ContainersEnginesResponse>, Status> {
        Ok(Response::new(wire::ContainersEnginesResponse::default()))
    }

    async fn set_engine(
        &self,
        _: Request<wire::ContainersSetEngineRequest>,
    ) -> Result<Response<wire::ContainersSetEngineResponse>, Status> {
        Ok(Response::new(wire::ContainersSetEngineResponse::default()))
    }

    async fn list(
        &self,
        _: Request<wire::ContainersListRequest>,
    ) -> Result<Response<wire::ContainersListResponse>, Status> {
        Ok(Response::new(wire::ContainersListResponse::default()))
    }

    async fn act(
        &self,
        _: Request<wire::ContainersActRequest>,
    ) -> Result<Response<wire::ContainersActResponse>, Status> {
        Ok(Response::new(wire::ContainersActResponse {
            error: Some(common::Error {
                code: "PRECONDITION_FAILED".into(),
                message: "pingora-panel-control-1 belongs to the panel's installation".into(),
                retryable: false,
                diagnostics: Vec::new(),
            }),
            ..wire::ContainersActResponse::default()
        }))
    }

    async fn inspect(
        &self,
        request: Request<wire::ContainersInspectRequest>,
    ) -> Result<Response<wire::ContainersInspectResponse>, Status> {
        let request = request.into_inner();
        assert_eq!(
            (request.engine.as_str(), request.container.as_str()),
            ("docker", "shop-web-1")
        );
        Ok(Response::new(wire::ContainersInspectResponse {
            detail: Some(wire::ContainerDetail {
                container: Some(wire::Container {
                    id: "b2".into(),
                    names: vec!["shop-web-1".into()],
                    state: wire::ContainerState::Running.into(),
                    ..wire::Container::default()
                }),
                restart_policy: "unless-stopped".into(),
                hostname: "web".into(),
                mounts: vec![wire::ContainerMount {
                    r#type: "bind".into(),
                    source: "/srv/shop".into(),
                    destination: "/usr/share/nginx/html".into(),
                    read_write: true,
                    ..wire::ContainerMount::default()
                }],
                networks: vec![wire::ContainerNetwork {
                    name: "shop_default".into(),
                    ip_address: "172.18.0.2".into(),
                    ..wire::ContainerNetwork::default()
                }],
                ..wire::ContainerDetail::default()
            }),
            error: None,
        }))
    }

    async fn logs(
        &self,
        request: Request<wire::ContainersLogsRequest>,
    ) -> Result<Response<wire::ContainersLogsResponse>, Status> {
        let request = request.into_inner();
        assert_eq!(
            (request.container.as_str(), request.lines),
            ("shop-web-1", 2)
        );
        assert_eq!(request.since.unwrap().seconds, 1_800_000_000);
        Ok(Response::new(wire::ContainersLogsResponse {
            observed_at: Some(at(10).into()),
            lines: vec![
                line(1, wire::ContainerLogStream::Stdout, "GET / 200"),
                line(2, wire::ContainerLogStream::Stderr, "upstream timed out"),
            ],
            truncated: true,
            error: None,
        }))
    }

    async fn stats(
        &self,
        request: Request<wire::ContainersStatsRequest>,
    ) -> Result<Response<wire::ContainersStatsResponse>, Status> {
        assert_eq!(
            request.into_inner().container,
            "",
            "every running container"
        );
        Ok(Response::new(wire::ContainersStatsResponse {
            observed_at: Some(at(10).into()),
            stats: vec![wire::ContainerStats {
                id: "b2".into(),
                name: "shop-web-1".into(),
                read_at: Some(at(9).into()),
                cpu_percent: 12.5,
                online_cpus: 4,
                memory_bytes: 200,
                memory_limit_bytes: 1_000,
                network: Some(wire::ContainerNetworkStats {
                    received_bytes: 1_500,
                    errors: 1,
                    ..wire::ContainerNetworkStats::default()
                }),
                block_read_bytes: 4_096,
                block_written_bytes: 8_192,
                pids: 5,
            }],
            error: None,
        }))
    }

    type FollowLogsStream =
        tokio_stream::Iter<std::vec::IntoIter<Result<wire::ContainersFollowLogsResponse, Status>>>;

    async fn follow_logs(
        &self,
        request: Request<wire::ContainersFollowLogsRequest>,
    ) -> Result<Response<Self::FollowLogsStream>, Status> {
        let request = request.into_inner();
        assert_eq!(request.lines, 0, "a resumed follow names no lines");
        assert_eq!(request.after.unwrap().seconds, 1_800_000_002);
        Ok(Response::new(tokio_stream::iter(vec![
            Ok(wire::ContainersFollowLogsResponse {
                lines: vec![line(3, 99, "GET /new 200")],
                error: None,
            }),
            Ok(wire::ContainersFollowLogsResponse {
                lines: Vec::new(),
                error: Some(common::Error {
                    code: "UNAVAILABLE".into(),
                    message: "the engine went away".into(),
                    retryable: true,
                    diagnostics: Vec::new(),
                }),
            }),
        ])))
    }
}

fn at(second: u64) -> std::time::SystemTime {
    std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_800_000_000 + second)
}

fn line(second: u64, stream: impl Into<i32>, text: &str) -> wire::ContainerLogLine {
    wire::ContainerLogLine {
        time: Some(at(second).into()),
        stream: stream.into(),
        text: text.into(),
    }
}

async fn client(fail: bool) -> OpsAgentClient {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(
        Server::builder()
            .add_service(AgentServer::new(FakeAgent))
            .add_service(DirectoriesServer::new(FakeDirectories { fail }))
            .add_service(ListenersServer::new(FakeListeners))
            .add_service(GatewayServiceServer::new(FakeGateway))
            .add_service(ContainersServer::new(FakeContainers))
            .add_service(ImagesServer::new(FakeImages))
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
async fn what_holds_the_web_ports_reaches_the_application() {
    let report = client(false)
        .await
        .listeners(scope(), vec![80, 443])
        .await
        .unwrap();
    assert!(report.observed_at.is_some());
    let listener = &report.listeners[0];
    assert_eq!((listener.address.as_str(), listener.port), ("0.0.0.0", 443));
    assert_eq!(listener.processes[0].name, "nginx");
    assert_eq!(listener.processes[0].pid, 812);
}

#[tokio::test]
async fn the_gateway_service_is_read_and_changed_through_the_agent() {
    let client = client(false).await;
    let status = client.gateway_service(scope()).await.unwrap();
    assert!(status.observed_at.is_some());
    assert_eq!(status.container.engine, "docker");
    assert_eq!(status.container.container.state, ContainerState::Running);
    assert_eq!(
        status.container.container.compose_project.as_deref(),
        Some("pingora-panel")
    );
    assert_eq!(status.container.health, "healthy");
    let context = CommandContext::new(
        RequestId::new("request-2").unwrap(),
        RequestId::new("request-2").unwrap(),
        "ops",
        RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
        IdempotencyKey::new("key-2").unwrap(),
    )
    .unwrap();
    let restarted = client
        .change_gateway_service(context.clone(), GatewayServiceAction::Restart)
        .await
        .unwrap();
    assert_eq!(restarted.container.restarts, 1);
    let refused = client
        .change_gateway_service(context, GatewayServiceAction::Stop)
        .await
        .unwrap_err();
    assert_eq!(refused.code.as_str(), "PRECONDITION_FAILED");
}

#[tokio::test]
async fn the_agents_errors_keep_their_codes() {
    let refused = client(true).await.directories(scope()).await.unwrap_err();
    assert_eq!(refused.code.as_str(), "STORAGE_UNAVAILABLE");
    assert!(refused.retryable);
}

#[tokio::test]
async fn containers_are_inspected_and_refusals_keep_their_codes() {
    let client = client(false).await;
    let detail = client
        .inspect(scope(), "docker".into(), "shop-web-1".into())
        .await
        .unwrap();
    assert_eq!(detail.container.names, vec!["shop-web-1".to_owned()]);
    assert_eq!(detail.container.state, ContainerState::Running);
    assert_eq!(detail.restart_policy.as_deref(), Some("unless-stopped"));
    assert_eq!(detail.user, None, "an empty value is unknown");
    assert_eq!(detail.mounts[0].name, None);
    assert!(detail.mounts[0].read_write);
    assert_eq!(detail.networks[0].ip_address.as_deref(), Some("172.18.0.2"));
    assert_eq!(detail.networks[0].gateway, None);

    let context = CommandContext::new(
        RequestId::new("request-3").unwrap(),
        RequestId::new("request-3").unwrap(),
        "ops",
        RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
        IdempotencyKey::new("key-3").unwrap(),
    )
    .unwrap();
    let refused = client
        .act(
            context,
            "docker".into(),
            "pingora-panel-control-1".into(),
            ContainerAction::Stop,
        )
        .await
        .unwrap_err();
    assert_eq!(refused.code.as_str(), "PRECONDITION_FAILED");
}

#[tokio::test]
async fn container_logs_are_read_and_followed() {
    use tokio_stream::StreamExt;

    let client = client(false).await;
    let logs = client
        .logs(
            scope(),
            "docker".into(),
            "shop-web-1".into(),
            ContainerLogQuery {
                lines: 2,
                since: Some(at(0)),
            },
        )
        .await
        .unwrap();
    assert!(logs.truncated);
    assert_eq!(logs.observed_at, Some(at(10)));
    assert_eq!(logs.lines[0].time, at(1));
    assert_eq!(
        (logs.lines[1].stream, logs.lines[1].text.as_str()),
        (ContainerLogStream::Stderr, "upstream timed out")
    );

    let followed: Vec<_> = client
        .follow_logs(
            scope(),
            "docker".into(),
            "shop-web-1".into(),
            ContainerLogStart::After(at(2)),
        )
        .await
        .unwrap()
        .collect()
        .await;
    let first = followed[0].as_ref().unwrap();
    assert_eq!(first[0].text, "GET /new 200");
    assert_eq!(
        first[0].stream,
        ContainerLogStream::Stdout,
        "a stream this build does not know is standard output"
    );
    assert_eq!(
        followed[1].as_ref().unwrap_err().code.as_str(),
        "UNAVAILABLE"
    );
}

#[tokio::test]
async fn running_containers_report_what_they_use() {
    let client = client(false).await;
    let list = client.stats(scope(), "docker".into(), None).await.unwrap();
    assert_eq!(list.observed_at, Some(at(10)));
    let web = &list.stats[0];
    assert_eq!(
        (web.name.as_str(), web.read_at),
        ("shop-web-1", Some(at(9)))
    );
    assert_eq!((web.cpu_percent, web.online_cpus), (12.5, 4));
    assert_eq!(
        (web.memory_bytes, web.memory_limit_bytes, web.pids),
        (200, 1_000, 5)
    );
    assert_eq!(
        web.network,
        Some(ContainerNetworkStats {
            received_bytes: 1_500,
            errors: 1,
            ..ContainerNetworkStats::default()
        })
    );
    assert_eq!(
        (web.block_read_bytes, web.block_written_bytes),
        (4_096, 8_192)
    );
}

#[tokio::test]
async fn images_are_listed_inspected_and_refused_removal() {
    let client = client(false).await;
    let list = client
        .images(scope(), "docker".into(), "nginx".into())
        .await
        .unwrap();
    assert_eq!(list.observed_at, Some(at(10)));
    let nginx = &list.images[0];
    assert_eq!(
        (nginx.tags[0].as_str(), nginx.containers),
        ("nginx:1.27", 1)
    );
    assert_eq!(nginx.created, Some(at(0)));
    assert_eq!(nginx.labels["maintainer"], "NGINX");

    let detail = client
        .inspect_image(scope(), "docker".into(), "nginx:1.27".into())
        .await
        .unwrap();
    assert_eq!(detail.architecture.as_deref(), Some("arm64"));
    assert_eq!(
        detail.os, None,
        "an empty value is one the agent does not know"
    );
    assert_eq!((detail.exposed_ports.len(), detail.layers), (1, 7));

    let context = CommandContext::new(
        RequestId::new("request-1").unwrap(),
        RequestId::new("request-1").unwrap(),
        "ops",
        RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
        IdempotencyKey::new("key-1").unwrap(),
    )
    .unwrap();
    let refused = client
        .remove_image(context, "docker".into(), "nginx:1.27".into(), true)
        .await
        .unwrap_err();
    assert_eq!(refused.code.as_str(), "CONFLICT");
}
