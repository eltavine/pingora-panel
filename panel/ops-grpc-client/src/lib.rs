#![forbid(unsafe_code)]

//! `HostAgentPort` over `ops-agent` (ADR 0030). The caller supplies the
//! channel, authenticated over the agent's socket.

use async_trait::async_trait;
use panel_application::{
    AgentCapability, AgentDescription, CapabilityState, CapabilityStatus, CommandContext,
    ComposeAction, ComposeChange, ComposeFailure, ComposeFile, ComposeLogLine, ComposeLogs,
    ComposePort, ComposeProject, ComposeProjectList, ContainerAction, ContainerAddress,
    ContainerChange, ContainerDetail, ContainerEngine, ContainerFilter, ContainerList,
    ContainerLogLine, ContainerLogQuery, ContainerLogStart, ContainerLogStream, ContainerLogTail,
    ContainerLogs, ContainerMount, ContainerNetwork, ContainerNetworkStats, ContainerState,
    ContainerStats, ContainerStatsList, ContainerSummary, ContainersPort, DirectoriesReport,
    DirectoryKind, DirectoryUsage, EngineDiskUsage, EngineDiskUse, EngineInfo, EngineNetwork,
    EngineNetworkList, EngineResourcesPort, EngineSubnet, EngineVersion, EngineVolume,
    EngineVolumeList, GatewayContainer, GatewayServiceAction, GatewayServiceStatus, HostAgentPort,
    Image, ImageDetail, ImageLayerProgress, ImageLayerState, ImageList, ImagePull, ImagePullEvent,
    ImagePullRequest, ImagePulled, ImageRemoval, ImagesPort, ListenersReport, ListeningProcess,
    PortListener, PortMapping, ProjectService, PruneChoices, PruneItem, PruneKind, PruneOutcome,
    PrunePreview, PruneReport, RequestScope,
};
use panel_contracts::ops::v1::{
    self as wire, agent_client::AgentClient, compose_projects_client::ComposeProjectsClient,
    containers_client::ContainersClient, directories_client::DirectoriesClient,
    engine_resources_client::EngineResourcesClient, gateway_service_client::GatewayServiceClient,
    gateway_service_status::Supervisor, images_client::ImagesClient,
    listeners_client::ListenersClient,
};
use panel_errors::{PanelError, Result};
use panel_service::{
    command_context, propagate_trace, request_context, response_error, status_error,
    GrpcHealthCheck,
};
use plugin_contracts::PLUGIN_METADATA;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio_stream::StreamExt;
use tonic::{metadata::AsciiMetadataValue, transport::Channel};

/// Longer than the agent's own limit on a directory walk.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Longer than the agent waits for the engine to stop the gateway.
const CHANGE_TIMEOUT: Duration = Duration::from_secs(150);
/// Longer than the agent gives an engine to add up its disk use.
const USAGE_TIMEOUT: Duration = Duration::from_secs(75);
/// Longer than the agent gives a prune preview.
const PREVIEW_TIMEOUT: Duration = Duration::from_secs(125);

#[derive(Clone)]
pub struct OpsAgentClient {
    channel: Channel,
    /// The plugin every call names, when the client reaches the container
    /// engine a plugin provides rather than the agent.
    plugin: Option<AsciiMetadataValue>,
}

impl OpsAgentClient {
    pub fn from_channel(channel: Channel) -> Self {
        Self {
            channel,
            plugin: None,
        }
    }

    /// A client of the container engines `plugin` provides through its
    /// container engine port, which the plugins module at `channel` serves
    /// (ADR 0044).
    pub fn for_plugin(channel: Channel, plugin: &str) -> Result<Self> {
        let plugin = plugin.parse().map_err(|_| {
            PanelError::invalid_argument(format!("{plugin:?} is not a plugin name"))
        })?;
        Ok(Self {
            channel,
            plugin: Some(plugin),
        })
    }

    fn named<T>(&self, mut request: tonic::Request<T>) -> tonic::Request<T> {
        if let Some(plugin) = &self.plugin {
            request
                .metadata_mut()
                .insert(PLUGIN_METADATA, plugin.clone());
        }
        request
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
        self.named(request)
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
        addresses: value
            .addresses
            .into_iter()
            .map(|address| ContainerAddress {
                network: address.network,
                ipv4: address.ipv4.parse().ok(),
                ipv6: address.ipv6.parse().ok(),
            })
            .collect(),
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

    async fn logs(
        &self,
        scope: RequestScope,
        engine_id: String,
        container: String,
        query: ContainerLogQuery,
    ) -> Result<ContainerLogs> {
        let message = wire::ContainersLogsRequest {
            context: Some(request_context(&scope)),
            engine: engine_id,
            container,
            lines: query.lines,
            since: query.since.map(Into::into),
        };
        let response = ContainersClient::new(self.channel.clone())
            .logs(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(ContainerLogs {
            observed_at: time(response.observed_at),
            lines: response.lines.into_iter().map(log_line).collect(),
            truncated: response.truncated,
        })
    }

    async fn follow_logs(
        &self,
        scope: RequestScope,
        engine_id: String,
        container: String,
        start: ContainerLogStart,
    ) -> Result<ContainerLogTail> {
        let (lines, after) = match start {
            ContainerLogStart::Last(lines) => (lines, None),
            ContainerLogStart::After(after) => (0, Some(after.into())),
        };
        let message = wire::ContainersFollowLogsRequest {
            context: Some(request_context(&scope)),
            engine: engine_id,
            container,
            lines,
            after,
        };
        // Following lasts as long as its reader, so it has no deadline.
        let mut request = tonic::Request::new(message);
        propagate_trace(request.metadata_mut(), scope.trace_context());
        let stream = ContainersClient::new(self.channel.clone())
            .follow_logs(self.named(request))
            .await
            .map_err(status_error)?
            .into_inner();
        Ok(Box::pin(stream.map(|message| {
            let message = message.map_err(status_error)?;
            response_error(message.error)?;
            Ok(message.lines.into_iter().map(log_line).collect())
        })))
    }

    async fn stats(
        &self,
        scope: RequestScope,
        engine_id: String,
        container: Option<String>,
    ) -> Result<ContainerStatsList> {
        let message = wire::ContainersStatsRequest {
            context: Some(request_context(&scope)),
            engine: engine_id,
            container: container.unwrap_or_default(),
        };
        let response = ContainersClient::new(self.channel.clone())
            .stats(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(ContainerStatsList {
            observed_at: time(response.observed_at),
            stats: response.stats.into_iter().map(stats).collect(),
        })
    }
}

fn stats(value: wire::ContainerStats) -> ContainerStats {
    ContainerStats {
        id: value.id,
        name: value.name,
        read_at: time(value.read_at),
        cpu_percent: value.cpu_percent,
        online_cpus: value.online_cpus,
        memory_bytes: value.memory_bytes,
        memory_limit_bytes: value.memory_limit_bytes,
        network: value.network.map(|network| ContainerNetworkStats {
            received_bytes: network.received_bytes,
            sent_bytes: network.sent_bytes,
            received_packets: network.received_packets,
            sent_packets: network.sent_packets,
            errors: network.errors,
            dropped: network.dropped,
        }),
        block_read_bytes: value.block_read_bytes,
        block_written_bytes: value.block_written_bytes,
        pids: value.pids,
    }
}

/// A line from a stream this build does not know counts as standard output.
fn log_line(value: wire::ContainerLogLine) -> ContainerLogLine {
    ContainerLogLine {
        time: time(value.time).unwrap_or(UNIX_EPOCH),
        stream: match wire::ContainerLogStream::try_from(value.stream) {
            Ok(wire::ContainerLogStream::Stderr) => ContainerLogStream::Stderr,
            _ => ContainerLogStream::Stdout,
        },
        text: value.text,
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

fn image_of(value: wire::Image) -> Image {
    Image {
        id: value.id,
        tags: value.tags,
        digests: value.digests,
        created: time(value.created),
        size_bytes: value.size_bytes,
        containers: value.containers,
        labels: value.labels.into_iter().collect(),
    }
}

#[async_trait]
impl ImagesPort for OpsAgentClient {
    async fn images(
        &self,
        scope: RequestScope,
        engine_id: String,
        search: String,
    ) -> Result<ImageList> {
        let message = wire::ImagesListRequest {
            context: Some(request_context(&scope)),
            engine: engine_id,
            search,
        };
        let response = ImagesClient::new(self.channel.clone())
            .list(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(ImageList {
            observed_at: time(response.observed_at),
            images: response.images.into_iter().map(image_of).collect(),
        })
    }

    async fn inspect_image(
        &self,
        scope: RequestScope,
        engine_id: String,
        image: String,
    ) -> Result<ImageDetail> {
        let message = wire::ImagesInspectRequest {
            context: Some(request_context(&scope)),
            engine: engine_id,
            image,
        };
        let response = ImagesClient::new(self.channel.clone())
            .inspect(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        let value = response.detail.unwrap_or_default();
        Ok(ImageDetail {
            image: image_of(value.image.unwrap_or_default()),
            architecture: known(value.architecture),
            variant: known(value.variant),
            os: known(value.os),
            author: known(value.author),
            comment: known(value.comment),
            user: known(value.user),
            working_directory: known(value.working_directory),
            exposed_ports: value.exposed_ports,
            volumes: value.volumes,
            stop_signal: known(value.stop_signal),
            layers: value.layers,
        })
    }

    async fn remove_image(
        &self,
        context: CommandContext,
        engine_id: String,
        image: String,
        force: bool,
    ) -> Result<ImageRemoval> {
        let message = wire::ImagesRemoveRequest {
            context: Some(command_context(&context)),
            engine: engine_id,
            image,
            force,
        };
        let mut request = self.request(message, &context.scope());
        request.set_timeout(CHANGE_TIMEOUT);
        let response = ImagesClient::new(self.channel.clone())
            .remove(request)
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(ImageRemoval {
            id: response.id,
            untagged: response.untagged,
            deleted: response.deleted,
        })
    }

    async fn pull_image(
        &self,
        context: CommandContext,
        engine_id: String,
        request: ImagePullRequest,
    ) -> Result<ImagePull> {
        let message = wire::ImagesPullRequest {
            context: Some(command_context(&context)),
            engine: engine_id,
            reference: request.reference,
            platform: request.platform.unwrap_or_default(),
            credentials: request
                .credentials
                .map(|credentials| wire::RegistryCredentials {
                    username: credentials.username,
                    password: credentials.password.as_str().to_owned(),
                }),
        };
        // The agent bounds how long a pull takes, so it has no deadline here.
        let mut request = tonic::Request::new(message);
        propagate_trace(request.metadata_mut(), context.trace_context());
        let stream = ImagesClient::new(self.channel.clone())
            .pull(self.named(request))
            .await
            .map_err(status_error)?
            .into_inner();
        Ok(Box::pin(stream.map(|message| {
            let message = message.map_err(status_error)?;
            response_error(message.error)?;
            Ok(match message.pulled {
                Some(pulled) => ImagePullEvent::Pulled(ImagePulled {
                    image: image_of(pulled.image.unwrap_or_default()),
                    digest: label(pulled.digest),
                    updated: pulled.updated,
                }),
                None => {
                    ImagePullEvent::Progress(message.layers.into_iter().map(layer_of).collect())
                }
            })
        })))
    }
}

fn layer_of(value: wire::ImageLayerProgress) -> ImageLayerProgress {
    ImageLayerProgress {
        state: match wire::ImageLayerState::try_from(value.state) {
            Ok(wire::ImageLayerState::Downloading) => ImageLayerState::Downloading,
            Ok(wire::ImageLayerState::Downloaded) => ImageLayerState::Downloaded,
            Ok(wire::ImageLayerState::Extracting) => ImageLayerState::Extracting,
            Ok(wire::ImageLayerState::Complete) => ImageLayerState::Complete,
            Ok(wire::ImageLayerState::Exists) => ImageLayerState::Exists,
            _ => ImageLayerState::Waiting,
        },
        id: value.id,
        current_bytes: value.current_bytes,
        total_bytes: value.total_bytes,
    }
}

/// An empty value the agent sends for one it does not know.
fn label(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

#[async_trait]
impl EngineResourcesPort for OpsAgentClient {
    async fn networks(&self, scope: RequestScope, engine_id: String) -> Result<EngineNetworkList> {
        let message = wire::EngineResourcesListNetworksRequest {
            context: Some(request_context(&scope)),
            engine: engine_id,
        };
        let response = EngineResourcesClient::new(self.channel.clone())
            .list_networks(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(EngineNetworkList {
            observed_at: time(response.observed_at),
            networks: response
                .networks
                .into_iter()
                .map(|network| EngineNetwork {
                    id: network.id,
                    name: network.name,
                    driver: network.driver,
                    scope: network.scope,
                    created: time(network.created),
                    internal: network.internal,
                    ipv6: network.ipv6,
                    subnets: network
                        .subnets
                        .into_iter()
                        .map(|subnet| EngineSubnet {
                            subnet: subnet.subnet,
                            gateway: label(subnet.gateway),
                        })
                        .collect(),
                    containers: network.containers,
                    compose_project: label(network.compose_project),
                    labels: network.labels.into_iter().collect(),
                })
                .collect(),
        })
    }

    async fn volumes(&self, scope: RequestScope, engine_id: String) -> Result<EngineVolumeList> {
        let message = wire::EngineResourcesListVolumesRequest {
            context: Some(request_context(&scope)),
            engine: engine_id,
        };
        let response = EngineResourcesClient::new(self.channel.clone())
            .list_volumes(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(EngineVolumeList {
            observed_at: time(response.observed_at),
            volumes: response
                .volumes
                .into_iter()
                .map(|volume| EngineVolume {
                    name: volume.name,
                    driver: volume.driver,
                    mountpoint: volume.mountpoint,
                    created: time(volume.created),
                    scope: volume.scope,
                    containers: volume.containers,
                    compose_project: label(volume.compose_project),
                    labels: volume.labels.into_iter().collect(),
                })
                .collect(),
        })
    }

    async fn disk_usage(&self, scope: RequestScope, engine_id: String) -> Result<EngineDiskUsage> {
        let message = wire::EngineResourcesDiskUsageRequest {
            context: Some(request_context(&scope)),
            engine: engine_id,
        };
        let mut request = self.request(message, &scope);
        request.set_timeout(USAGE_TIMEOUT);
        let response = EngineResourcesClient::new(self.channel.clone())
            .disk_usage(request)
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        let use_of = |value: Option<wire::EngineDiskUse>| {
            let value = value.unwrap_or_default();
            EngineDiskUse {
                total: value.total,
                active: value.active,
                size_bytes: value.size_bytes,
                reclaimable_bytes: value.reclaimable_bytes,
            }
        };
        Ok(EngineDiskUsage {
            observed_at: time(response.observed_at),
            images: use_of(response.images),
            containers: use_of(response.containers),
            volumes: use_of(response.volumes),
            build_cache: use_of(response.build_cache),
        })
    }

    async fn prune_preview(
        &self,
        scope: RequestScope,
        engine_id: String,
        choices: PruneChoices,
    ) -> Result<PrunePreview> {
        let message = wire::EngineResourcesPrunePreviewRequest {
            context: Some(request_context(&scope)),
            engine: engine_id,
            choices: Some(wire_choices(choices)),
        };
        let mut request = self.request(message, &scope);
        request.set_timeout(PREVIEW_TIMEOUT);
        let response = EngineResourcesClient::new(self.channel.clone())
            .prune_preview(request)
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(PrunePreview {
            observed_at: time(response.observed_at),
            items: response.items.into_iter().filter_map(prune_item).collect(),
            reclaimable_bytes: response.reclaimable_bytes,
        })
    }

    async fn prune(
        &self,
        context: CommandContext,
        engine_id: String,
        choices: PruneChoices,
        items: Vec<PruneItem>,
    ) -> Result<PruneReport> {
        let message = wire::EngineResourcesPruneRequest {
            context: Some(command_context(&context)),
            engine: engine_id,
            choices: Some(wire_choices(choices)),
            items: items.into_iter().map(wire_item).collect(),
        };
        let mut request = self.request(message, &context.scope());
        request.set_timeout(CHANGE_TIMEOUT);
        let response = EngineResourcesClient::new(self.channel.clone())
            .prune(request)
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(PruneReport {
            outcomes: response
                .outcomes
                .into_iter()
                .filter_map(|outcome| {
                    Some(PruneOutcome {
                        item: prune_item(outcome.item?)?,
                        refusal: outcome.error.map(PanelError::from),
                    })
                })
                .collect(),
            reclaimed_bytes: response.reclaimed_bytes,
        })
    }
}

fn wire_choices(choices: PruneChoices) -> wire::EnginePruneChoices {
    wire::EnginePruneChoices {
        tagged_images: choices.tagged_images,
        named_volumes: choices.named_volumes,
    }
}

fn wire_item(item: PruneItem) -> wire::EnginePruneItem {
    let kind = match item.kind {
        PruneKind::Container => wire::EnginePruneKind::Container,
        PruneKind::Image => wire::EnginePruneKind::Image,
        PruneKind::Volume => wire::EnginePruneKind::Volume,
        PruneKind::Network => wire::EnginePruneKind::Network,
        PruneKind::BuildCache => wire::EnginePruneKind::BuildCache,
    };
    wire::EnginePruneItem {
        kind: kind.into(),
        id: item.id,
        name: item.name,
        size_bytes: item.size_bytes,
    }
}

/// An item of a kind this build does not know is left out: nothing here
/// could ask to remove it.
fn prune_item(value: wire::EnginePruneItem) -> Option<PruneItem> {
    let kind = match wire::EnginePruneKind::try_from(value.kind).ok()? {
        wire::EnginePruneKind::Container => PruneKind::Container,
        wire::EnginePruneKind::Image => PruneKind::Image,
        wire::EnginePruneKind::Volume => PruneKind::Volume,
        wire::EnginePruneKind::Network => PruneKind::Network,
        wire::EnginePruneKind::BuildCache => PruneKind::BuildCache,
        wire::EnginePruneKind::Unspecified => return None,
    };
    Some(PruneItem {
        kind,
        id: value.id,
        name: value.name,
        size_bytes: value.size_bytes,
    })
}

fn compose_project(value: wire::ComposeProject) -> ComposeProject {
    ComposeProject {
        name: value.name,
        working_directory: known(value.working_directory),
        config_files: value.config_files,
        services: value
            .services
            .into_iter()
            .map(|service| ProjectService {
                name: service.name,
                containers: service.containers,
                running: service.running,
            })
            .collect(),
        containers: value.containers,
        running: value.running,
        installation: value.installation,
    }
}

#[async_trait]
impl ComposePort for OpsAgentClient {
    async fn projects(&self, scope: RequestScope, engine_id: String) -> Result<ComposeProjectList> {
        let message = wire::ComposeProjectsListRequest {
            context: Some(request_context(&scope)),
            engine: engine_id,
        };
        let response = ComposeProjectsClient::new(self.channel.clone())
            .list(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(ComposeProjectList {
            observed_at: time(response.observed_at),
            projects: response.projects.into_iter().map(compose_project).collect(),
        })
    }

    async fn act_on_project(
        &self,
        context: CommandContext,
        engine_id: String,
        project: String,
        action: ComposeAction,
    ) -> Result<ComposeChange> {
        let action = match action {
            ComposeAction::Up => wire::ComposeAction::Up,
            ComposeAction::Down => wire::ComposeAction::Down,
            ComposeAction::Restart => wire::ComposeAction::Restart,
        };
        let message = wire::ComposeProjectsActRequest {
            context: Some(command_context(&context)),
            engine: engine_id,
            project,
            action: action.into(),
        };
        let mut request = self.request(message, &context.scope());
        request.set_timeout(CHANGE_TIMEOUT);
        let response = ComposeProjectsClient::new(self.channel.clone())
            .act(request)
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(ComposeChange {
            project: response.project.map(compose_project),
            changed: response.changed,
            failures: response
                .failures
                .into_iter()
                .map(|failure| ComposeFailure {
                    name: failure.name,
                    error: failure.error.map_or_else(
                        || PanelError::internal("the engine refused without saying why"),
                        PanelError::from,
                    ),
                })
                .collect(),
        })
    }

    async fn project_logs(
        &self,
        scope: RequestScope,
        engine_id: String,
        project: String,
        query: ContainerLogQuery,
    ) -> Result<ComposeLogs> {
        let message = wire::ComposeProjectsLogsRequest {
            context: Some(request_context(&scope)),
            engine: engine_id,
            project,
            lines: query.lines,
            since: query.since.map(Into::into),
        };
        let response = ComposeProjectsClient::new(self.channel.clone())
            .logs(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(ComposeLogs {
            observed_at: time(response.observed_at),
            lines: response
                .lines
                .into_iter()
                .map(|line| ComposeLogLine {
                    service: line.service,
                    container: line.container,
                    line: log_line(line.line.unwrap_or_default()),
                })
                .collect(),
            truncated: response.truncated,
        })
    }

    async fn project_files(
        &self,
        scope: RequestScope,
        engine_id: String,
        project: String,
    ) -> Result<Vec<ComposeFile>> {
        let message = wire::ComposeProjectsFilesRequest {
            context: Some(request_context(&scope)),
            engine: engine_id,
            project,
        };
        let response = ComposeProjectsClient::new(self.channel.clone())
            .files(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(response
            .files
            .into_iter()
            .map(|file| ComposeFile {
                path: file.path,
                content: match file.error {
                    Some(error) => Err(PanelError::from(error)),
                    None => Ok(file.content),
                },
            })
            .collect())
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
