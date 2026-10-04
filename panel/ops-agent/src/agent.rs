use crate::AgentConfig;
use panel_contracts::{
    common::v1 as common,
    ops::v1::{
        self as wire, agent_server::Agent, AgentCapability, AgentDescription, Capability,
        CapabilityState,
    },
    OPS_V1,
};
use tonic::{Request, Response, Status};

/// What the agent is and which capabilities it has.
pub(crate) struct AgentService {
    description: AgentDescription,
}

impl AgentService {
    pub(crate) fn new(capabilities: Vec<AgentCapability>) -> Self {
        let capability_set = capabilities
            .iter()
            .filter(|capability| capability.state() == CapabilityState::Available)
            .map(|capability| format!("{}@1", name(capability.capability())))
            .collect::<Vec<_>>()
            .join(",");
        Self {
            description: AgentDescription {
                version: Some(common::Version {
                    product: "pingora-panel".into(),
                    component: crate::SERVICE.into(),
                    build: env!("CARGO_PKG_VERSION").into(),
                    schema: String::new(),
                    protocol: format!("{}@{}..={}", OPS_V1.package, OPS_V1.min, OPS_V1.max),
                    capability_set,
                }),
                hostname: hostname(),
                capabilities,
            },
        }
    }

    pub(crate) fn description(&self) -> &AgentDescription {
        &self.description
    }
}

#[tonic::async_trait]
impl Agent for AgentService {
    async fn describe(
        &self,
        _: Request<wire::AgentDescribeRequest>,
    ) -> Result<Response<wire::AgentDescribeResponse>, Status> {
        Ok(Response::new(wire::AgentDescribeResponse {
            description: Some(self.description.clone()),
            error: None,
        }))
    }
}

/// Whether directory sizes are enabled.
pub(crate) fn directories(config: &AgentConfig) -> AgentCapability {
    if config.directories.is_empty() {
        AgentCapability {
            capability: Capability::Directories.into(),
            state: CapabilityState::NotEnabled.into(),
            detail: format!(
                "set {}, {} or {}",
                crate::config::CONFIGURATION_DIR_ENV,
                crate::config::LOGS_DIR_ENV,
                crate::config::CERTIFICATES_DIR_ENV
            ),
        }
    } else {
        available(Capability::Directories)
    }
}

fn available(capability: Capability) -> AgentCapability {
    AgentCapability {
        capability: capability.into(),
        state: CapabilityState::Available.into(),
        detail: String::new(),
    }
}

/// The capability as `capability_set` names it.
fn name(capability: Capability) -> &'static str {
    match capability {
        Capability::Unspecified => "unspecified",
        Capability::Directories => "directories",
        Capability::Listeners => "listeners",
        Capability::GatewayUnit => "gateway-unit",
        Capability::Containers => "containers",
        Capability::GatewayService => "gateway-service",
    }
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|name| name.trim().to_owned())
        .unwrap_or_default()
}
