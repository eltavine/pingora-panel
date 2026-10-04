#![forbid(unsafe_code)]

//! `HostAgentPort` over `ops-agent` (ADR 0030). The caller supplies the
//! channel, authenticated over the agent's socket.

use async_trait::async_trait;
use panel_application::{
    AgentCapability, AgentDescription, CapabilityState, CapabilityStatus, DirectoriesReport,
    DirectoryKind, DirectoryUsage, HostAgentPort, ListenersReport, ListeningProcess, PortListener,
    RequestScope,
};
use panel_contracts::ops::v1::{
    self as wire, agent_client::AgentClient, directories_client::DirectoriesClient,
    listeners_client::ListenersClient,
};
use panel_errors::Result;
use panel_service::{
    propagate_trace, request_context, response_error, status_error, GrpcHealthCheck,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tonic::transport::Channel;

/// Longer than the agent's own limit on a directory walk.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

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
        wire::Capability::GatewayUnit => Some(AgentCapability::GatewayUnit),
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
