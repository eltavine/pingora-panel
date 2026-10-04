#![forbid(unsafe_code)]

//! `HostAgentPort` over `ops-agent` (ADR 0030). The caller supplies the
//! channel, authenticated over the agent's socket.

use async_trait::async_trait;
use panel_application::{
    AgentCapability, AgentDescription, CapabilityState, CapabilityStatus, CommandContext,
    ContainerAction, ContainerChange, ContainerDetail, ContainerEngine, ContainerFilter,
    ContainerList, ContainerMount, ContainerNetwork, ContainerState, ContainerSummary,
    ContainersPort, DirectoriesReport, DirectoryKind, DirectoryUsage, EngineInfo, EngineVersion,
    GatewayContainer, GatewayServiceAction, GatewayServiceStatus, HostAgentPort, ListenersReport,
    ListeningProcess, PortListener, PortMapping, RequestScope,
};
use panel_contracts::{
    common::v1 as common,
    ops::v1::{
        self as wire, agent_client::AgentClient, containers_client::ContainersClient,
        directories_client::DirectoriesClient, gateway_service_client::GatewayServiceClient,
        gateway_service_status::Supervisor, listeners_client::ListenersClient,
    },
    PROTOCOL_VERSION,
};
use panel_errors::{PanelError, Result};
use panel_service::{
    propagate_trace, request_context, response_error, status_error, GrpcHealthCheck,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tonic::transport::Channel;

/// Longer than the agent's own limit on a directory walk.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Longer than the agent waits for the engine to stop the gateway.
const CHANGE_TIMEOUT: Duration = Duration::from_secs(150);

#[derive(Clone)]
pub struct OpsAgentClient {
    channel: Channel,
}

impl OpsAgentClient {
    pub fn from_channel(channel: Channel) -> Self {
        Self { channel }
    }

    /// A readiness check against the agent's standard gRPC health.
    pub fn health_check(&self) -> GrpcHealthCheck {
        GrpcHealthCheck::new(
            "ops-agent",
            self.channel.clone(),
            wire::agent_server::SERVICE_NAME,
            REQUEST_TIMEOUT,
        )
    }

    fn request<T>(&self, message: T, scope: &RequestScope) -> tonic::Request<T> {
        let mut request = tonic::Request::new(message);
        request.set_timeout(REQUEST_TIMEOUT);
        propagate_trace(request.metadata_mut(), scope.trace_context());
        request
    }
}

fn time(value: Option<prost_types::Timestamp>) -> Option<SystemTime> {
    let value = value?;
    let seconds = u64::try_from(value.seconds).ok()?;
    let nanos = u32::try_from(value.nanos).ok()?;
    UNIX_EPOCH.checked_add(Duration::new(seconds, nanos))
}

/// A capability this build knows, or `None` for one a newer agent added.
fn capability(value: i32) -> Option<AgentCapability> {
    match wire::Capability::try_from(value).ok()? {
        wire::Capability::Directories => Some(AgentCapability::Directories),
        wire::Capability::Listeners => Some(AgentCapability::Listeners),
        wire::Capability::GatewayService => Some(AgentCapability::GatewayService),
        wire::Capability::Containers => Some(AgentCapability::Containers),
        wire::Capability::Unspecified => None,
    }
}

/// A state this build does not know is not taken as available.
fn state(value: i32) -> CapabilityState {
    match wire::CapabilityState::try_from(value) {
        Ok(wire::CapabilityState::Available) => CapabilityState::Available,
        Ok(wire::CapabilityState::NotEnabled) => CapabilityState::NotEnabled,
        Ok(wire::CapabilityState::Denied) => CapabilityState::Denied,
        Ok(wire::CapabilityState::Unreachable) => CapabilityState::Unreachable,
        Ok(wire::CapabilityState::Unsupported | wire::CapabilityState::Unspecified) | Err(_) => {
            CapabilityState::Unsupported
        }
    }
}

fn description(value: wire::AgentDescription) -> AgentDescription {
    let version = value.version.unwrap_or_default();
    AgentDescription {
        build: version.build,
        protocol: version.protocol,
        hostname: value.hostname,
        capabilities: value
            .capabilities
            .into_iter()
            .filter_map(|status| {
                Some(CapabilityStatus {
                    capability: capability(status.capability)?,
                    state: state(status.state),
                    detail: status.detail,
                })
            })
            .collect(),
    }
}

fn directory(value: wire::DirectoryUsage) -> Option<DirectoryUsage> {
    let kind = match wire::DirectoryKind::try_from(value.kind).ok()? {
        wire::DirectoryKind::Configuration => DirectoryKind::Configuration,
        wire::DirectoryKind::Logs => DirectoryKind::Logs,
        wire::DirectoryKind::Certificates => DirectoryKind::Certificates,
        wire::DirectoryKind::Unspecified => return None,
    };
    Some(DirectoryUsage {
        kind,
        path: value.path,
        present: value.present,
        bytes: value.bytes,
        files: value.files,
        unreadable: value.unreadable,
        truncated: value.truncated,
    })
}

fn command_context(context: &CommandContext) -> common::RequestContext {
    common::RequestContext {
        request_id: context.request_id().as_str().into(),
        correlation_id: context.correlation_id().as_str().into(),
        actor: context.actor().into(),
        deadline: context.deadline().as_str().into(),
        idempotency_key: context.idempotency_key().as_str().into(),
        schema_version: PROTOCOL_VERSION.into(),
        site_scope: None,
    }
}

fn gateway_service(value: Option<wire::GatewayServiceStatus>) -> Result<GatewayServiceStatus> {
    let value = value.unwrap_or_default();
    match value.supervisor {
        Some(Supervisor::Container(gateway)) => Ok(GatewayServiceStatus {
            observed_at: time(value.observed_at),
            container: GatewayContainer {
                engine: gateway.engine,
                container: summary(gateway.container.unwrap_or_default()),
                started_at: time(gateway.started_at),
                finished_at: time(gateway.finished_at),
                exit_code: gateway.exit_code,
                restarts: gateway.restarts,
                health: gateway.health,
            },
        }),
        None => Err(PanelError::unavailable(
            "the agent named nothing that runs the gateway",
        )),
    }
}

fn listener(value: wire::Listener) -> Option<PortListener> {
    Some(PortListener {
        address: value.address,
        port: u16::try_from(value.port).ok()?,
        uid: value.uid,
        processes: value
            .processes
            .into_iter()
            .map(|process| ListeningProcess {
                pid: process.pid,
                name: process.name,
                executable: process.executable,
                uid: process.uid,
            })
            .collect(),
    })
}

#[async_trait]
impl HostAgentPort for OpsAgentClient {
    async fn agent(&self, scope: RequestScope) -> Result<AgentDescription> {
        let message = wire::AgentDescribeRequest {
            context: Some(request_context(&scope)),
        };
        let response = AgentClient::new(self.channel.clone())
            .describe(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(description(response.description.unwrap_or_default()))
    }

    async fn directories(&self, scope: RequestScope) -> Result<DirectoriesReport> {
        let message = wire::DirectoriesUsageRequest {
            context: Some(request_context(&scope)),
        };
        let response = DirectoriesClient::new(self.channel.clone())
            .usage(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(DirectoriesReport {
            observed_at: time(response.observed_at),
            directories: response
                .directories
                .into_iter()
                .filter_map(directory)
                .collect(),
        })
    }

    async fn listeners(&self, scope: RequestScope, ports: Vec<u16>) -> Result<ListenersReport> {
        let message = wire::ListenersListRequest {
            context: Some(request_context(&scope)),
            ports: ports.into_iter().map(u32::from).collect(),
        };
        let response = ListenersClient::new(self.channel.clone())
            .list(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(ListenersReport {
            observed_at: time(response.observed_at),
            listeners: response
                .listeners
                .into_iter()
                .filter_map(listener)
                .collect(),
        })
    }

    async fn gateway_service(&self, scope: RequestScope) -> Result<GatewayServiceStatus> {
        let message = wire::GatewayServiceStatusRequest {
            context: Some(request_context(&scope)),
        };
        let response = GatewayServiceClient::new(self.channel.clone())
            .status(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        gateway_service(response.status)
    }

    async fn change_gateway_service(
        &self,
        context: CommandContext,
        action: GatewayServiceAction,
    ) -> Result<GatewayServiceStatus> {
        let action = match action {
            GatewayServiceAction::Start => wire::GatewayServiceAction::Start,
            GatewayServiceAction::Stop => wire::GatewayServiceAction::Stop,
            GatewayServiceAction::Restart => wire::GatewayServiceAction::Restart,
        };
        let message = wire::GatewayServiceChangeRequest {
            context: Some(command_context(&context)),
            action: action.into(),
        };
        let mut request = self.request(message, &context.scope());
        request.set_timeout(CHANGE_TIMEOUT);
        let response = GatewayServiceClient::new(self.channel.clone())
            .change(request)
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        gateway_service(response.status)
    }
}

fn engine(value: wire::Engine) -> ContainerEngine {
    ContainerEngine {
        id: value.id,
        socket: value.socket,
        enabled: value.enabled,
        reachable: value.reachable,
        detail: value.detail,
        version: value.version.map(|version| EngineVersion {
            version: version.version,
            api_version: version.api_version,
            os: version.os,
            architecture: version.architecture,
            kernel_version: version.kernel_version,
            go_version: version.go_version,
        }),
        info: value.info.map(|info| EngineInfo {
            containers: info.containers,
            running: info.running,
            paused: info.paused,
            stopped: info.stopped,
            images: info.images,
            storage_driver: info.storage_driver,
            cgroup_driver: info.cgroup_driver,
            operating_system: info.operating_system,
            cpus: info.cpus,
            memory_bytes: info.memory_bytes,
            name: info.name,
        }),
    }
}

fn container_state(value: i32) -> ContainerState {
    match wire::ContainerState::try_from(value) {
        Ok(wire::ContainerState::Created) => ContainerState::Created,
        Ok(wire::ContainerState::Running) => ContainerState::Running,
        Ok(wire::ContainerState::Paused) => ContainerState::Paused,
        Ok(wire::ContainerState::Restarting) => ContainerState::Restarting,
        Ok(wire::ContainerState::Exited) => ContainerState::Exited,
        Ok(wire::ContainerState::Removing) => ContainerState::Removing,
        Ok(wire::ContainerState::Dead) => ContainerState::Dead,
        Ok(wire::ContainerState::Stopping) => ContainerState::Stopping,
        Ok(wire::ContainerState::Unspecified) | Err(_) => ContainerState::Unknown,
    }
}

/// The state as the agent names it; a state this build does not know
/// matches nothing.
fn wire_state(value: ContainerState) -> Option<wire::ContainerState> {
    Some(match value {
        ContainerState::Created => wire::ContainerState::Created,
        ContainerState::Running => wire::ContainerState::Running,
        ContainerState::Paused => wire::ContainerState::Paused,
        ContainerState::Restarting => wire::ContainerState::Restarting,
        ContainerState::Exited => wire::ContainerState::Exited,
        ContainerState::Removing => wire::ContainerState::Removing,
        ContainerState::Dead => wire::ContainerState::Dead,
        ContainerState::Stopping => wire::ContainerState::Stopping,
        ContainerState::Unknown => return None,
    })
}

fn summary(value: wire::Container) -> ContainerSummary {
    ContainerSummary {
        id: value.id,
        names: value.names,
        image: value.image,
        image_id: value.image_id,
        created: time(value.created),
        state: container_state(value.state),
        status: value.status,
        ports: value
            .ports
            .into_iter()
            .map(|port| PortMapping {
                private_port: u16::try_from(port.private_port).unwrap_or(0),
                public_port: u16::try_from(port.public_port)
                    .ok()
                    .filter(|port| *port != 0),
                host_ip: port.host_ip,
                protocol: port.protocol,
            })
            .collect(),
        labels: value.labels.into_iter().collect(),
        compose_project: (!value.compose_project.is_empty()).then_some(value.compose_project),
    }
}

#[async_trait]
impl ContainersPort for OpsAgentClient {
    async fn engines(&self, scope: RequestScope) -> Result<Vec<ContainerEngine>> {
        let message = wire::ContainersEnginesRequest {
            context: Some(request_context(&scope)),
        };
        let response = ContainersClient::new(self.channel.clone())
            .engines(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(response.engines.into_iter().map(engine).collect())
    }

    async fn set_engine(
        &self,
        context: CommandContext,
        engine_id: String,
        enabled: bool,
    ) -> Result<ContainerEngine> {
        let message = wire::ContainersSetEngineRequest {
            context: Some(command_context(&context)),
            engine: engine_id,
            enabled,
        };
        let response = ContainersClient::new(self.channel.clone())
            .set_engine(self.request(message, &context.scope()))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(engine(response.engine.unwrap_or_default()))
    }

    async fn containers(
        &self,
        scope: RequestScope,
        engine_id: String,
        filter: ContainerFilter,
    ) -> Result<ContainerList> {
        let message = wire::ContainersListRequest {
            context: Some(request_context(&scope)),
            engine: engine_id,
            search: filter.search,
            states: filter
                .states
                .into_iter()
                .filter_map(wire_state)
                .map(Into::into)
                .collect(),
        };
        let response = ContainersClient::new(self.channel.clone())
            .list(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(ContainerList {
            observed_at: time(response.observed_at),
            containers: response.containers.into_iter().map(summary).collect(),
        })
    }

    async fn act(
        &self,
        context: CommandContext,
        engine_id: String,
        container: String,
        action: ContainerAction,
    ) -> Result<ContainerChange> {
        let (wire_action, force, remove_volumes) = match action {
            ContainerAction::Start => (wire::ContainerAction::Start, false, false),
            ContainerAction::Stop => (wire::ContainerAction::Stop, false, false),
            ContainerAction::Restart => (wire::ContainerAction::Restart, false, false),
            ContainerAction::Kill => (wire::ContainerAction::Kill, false, false),
            ContainerAction::Remove { force, volumes } => {
                (wire::ContainerAction::Remove, force, volumes)
            }
        };
        let message = wire::ContainersActRequest {
            context: Some(command_context(&context)),
            engine: engine_id,
            container,
            action: wire_action.into(),
            force,
            remove_volumes,
        };
        let mut request = self.request(message, &context.scope());
        request.set_timeout(CHANGE_TIMEOUT);
        let response = ContainersClient::new(self.channel.clone())
            .act(request)
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(ContainerChange {
            id: response.id,
            name: response.name,
            container: response.container.map(summary),
        })
    }

    async fn inspect(
        &self,
        scope: RequestScope,
        engine_id: String,
        container: String,
    ) -> Result<ContainerDetail> {
        let message = wire::ContainersInspectRequest {
            context: Some(request_context(&scope)),
            engine: engine_id,
            container,
        };
        let response = ContainersClient::new(self.channel.clone())
            .inspect(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(detail(response.detail.unwrap_or_default()))
    }
}

/// An empty value the agent sends for one it does not know.
fn known(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn detail(value: wire::ContainerDetail) -> ContainerDetail {
    ContainerDetail {
        container: summary(value.container.unwrap_or_default()),
        started_at: time(value.started_at),
        finished_at: time(value.finished_at),
        exit_code: value.exit_code,
        error: known(value.error),
        oom_killed: value.oom_killed,
        restarts: value.restarts,
        health: known(value.health),
        restart_policy: known(value.restart_policy),
        restart_retries: value.restart_retries,
        hostname: known(value.hostname),
        user: known(value.user),
        working_directory: known(value.working_directory),
        platform: known(value.platform),
        mounts: value
            .mounts
            .into_iter()
            .map(|mount| ContainerMount {
                kind: mount.r#type,
                name: known(mount.name),
                source: mount.source,
                destination: mount.destination,
                read_write: mount.read_write,
            })
            .collect(),
        networks: value
            .networks
            .into_iter()
            .map(|network| ContainerNetwork {
                name: network.name,
                ip_address: known(network.ip_address),
                ipv6_address: known(network.ipv6_address),
                gateway: known(network.gateway),
                mac_address: known(network.mac_address),
                aliases: network.aliases,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_from_a_newer_agent_are_skipped_or_not_trusted() {
        let described = description(wire::AgentDescription {
            version: None,
            hostname: "host".into(),
            capabilities: vec![
                wire::AgentCapability {
                    capability: wire::Capability::Directories.into(),
                    state: wire::CapabilityState::Available.into(),
                    detail: String::new(),
                },
                wire::AgentCapability {
                    capability: 99,
                    state: wire::CapabilityState::Available.into(),
                    detail: String::new(),
                },
                wire::AgentCapability {
                    capability: wire::Capability::Containers.into(),
                    state: 99,
                    detail: "new".into(),
                },
            ],
        });
        assert_eq!(described.capabilities.len(), 2);
        assert!(described.has(AgentCapability::Directories));
        assert_eq!(
            described.capabilities[1].state,
            CapabilityState::Unsupported
        );
        assert!(directory(wire::DirectoryUsage {
            kind: 99,
            ..wire::DirectoryUsage::default()
        })
        .is_none());
    }
}
