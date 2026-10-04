use crate::{
    access::{AccessAudit, AccessSettings, Gate},
    sign_in::ProviderAccess,
};
use panel_application::{
    AlertsPort, AuditPort, ContainersPort, GatewayRuntimePort, HostAgentPort, HostPort, LogsPort,
    NoContainers, NoHostAgent, TlsProbe, TrafficPort,
};
use panel_certificate_api::CertificatePort;
use panel_config_api::ConfigurationPort;
use panel_health::HealthWatch;
use panel_identity::{Identity, ProviderDirectory, ProviderSignIns, WorkloadIdentity};
use panel_platform::ServiceDirectory;
use std::sync::Arc;

pub struct ApiState<U> {
    pub(crate) use_cases: Arc<U>,
    pub(crate) health: Option<HealthWatch>,
    pub(crate) directory: Option<Arc<dyn ServiceDirectory>>,
    pub(crate) configuration: Option<Arc<dyn ConfigurationPort>>,
    pub(crate) runtime: Option<Arc<dyn GatewayRuntimePort>>,
    pub(crate) audit: Option<Arc<dyn AuditPort>>,
    pub(crate) certificates: Option<Arc<dyn CertificatePort>>,
    pub(crate) tls_probe: Option<Arc<dyn TlsProbe>>,
    pub(crate) traffic: Option<Arc<dyn TrafficPort>>,
    pub(crate) logs: Option<Arc<dyn LogsPort>>,
    pub(crate) alerts: Option<Arc<dyn AlertsPort>>,
    pub(crate) host: Option<Arc<dyn HostPort>>,
    pub(crate) host_agent: Arc<dyn HostAgentPort>,
    pub(crate) containers: Arc<dyn ContainersPort>,
    pub(crate) identity: Option<Arc<Gate>>,
    pub(crate) providers: Option<Arc<ProviderAccess>>,
    pub(crate) workloads: Option<Arc<WorkloadIdentity>>,
    pub(crate) access_audit: Option<Arc<dyn AccessAudit>>,
}

impl<U> Clone for ApiState<U> {
    fn clone(&self) -> Self {
        Self {
            use_cases: Arc::clone(&self.use_cases),
            health: self.health.clone(),
            directory: self.directory.clone(),
            configuration: self.configuration.clone(),
            runtime: self.runtime.clone(),
            audit: self.audit.clone(),
            certificates: self.certificates.clone(),
            tls_probe: self.tls_probe.clone(),
            traffic: self.traffic.clone(),
            logs: self.logs.clone(),
            alerts: self.alerts.clone(),
            host: self.host.clone(),
            host_agent: Arc::clone(&self.host_agent),
            containers: Arc::clone(&self.containers),
            identity: self.identity.clone(),
            providers: self.providers.clone(),
            workloads: self.workloads.clone(),
            access_audit: self.access_audit.clone(),
        }
    }
}

impl<U> ApiState<U> {
    pub fn new(use_cases: Arc<U>) -> Self {
        Self {
            use_cases,
            health: None,
            directory: None,
            configuration: None,
            runtime: None,
            audit: None,
            certificates: None,
            tls_probe: None,
            traffic: None,
            logs: None,
            alerts: None,
            host: None,
            host_agent: Arc::new(NoHostAgent),
            containers: Arc::new(NoContainers),
            identity: None,
            providers: None,
            workloads: None,
            access_audit: None,
        }
    }

    /// Manages identity providers, and signs people in through them once
    /// the panel knows the public origin they return to.
    pub fn with_identity_providers(
        mut self,
        directory: ProviderDirectory,
        sign_ins: Option<ProviderSignIns>,
    ) -> Self {
        self.providers = Some(Arc::new(ProviderAccess {
            directory,
            sign_ins,
        }));
        self
    }

    /// Keeps workload identities and exchanges workload tokens for
    /// sessions of service accounts.
    pub fn with_workload_identity(mut self, workloads: WorkloadIdentity) -> Self {
        self.workloads = Some(Arc::new(workloads));
        self
    }

    /// Records the requests the identity guard refuses to authenticated
    /// callers.
    pub fn with_access_audit(mut self, audit: Arc<dyn AccessAudit>) -> Self {
        self.access_audit = Some(audit);
        self
    }

    /// Authenticates every request and authorizes it by its route's access
    /// rule, recording the caller as the actor. Without it the API trusts
    /// its network and takes the actor from `x-actor`.
    pub fn with_identity(mut self, identity: Identity, settings: AccessSettings) -> Self {
        self.identity = Some(Gate::new(identity, settings));
        self
    }

    /// Serves data plane status, reloads, workers, shutdown and upstream health.
    pub fn with_runtime(mut self, runtime: Arc<dyn GatewayRuntimePort>) -> Self {
        self.runtime = Some(runtime);
        self
    }

    /// Serves the audit trail under `/api/v1/audit-events`.
    pub fn with_audit(mut self, audit: Arc<dyn AuditPort>) -> Self {
        self.audit = Some(audit);
        self
    }

    /// Serves what the gateway served under `/api/v1/traffic`.
    pub fn with_traffic(mut self, traffic: Arc<dyn TrafficPort>) -> Self {
        self.traffic = Some(traffic);
        self
    }

    /// Serves the gateway's logs under `/api/v1/logs`.
    pub fn with_logs(mut self, logs: Arc<dyn LogsPort>) -> Self {
        self.logs = Some(logs);
        self
    }

    /// Serves the host's figures under `/api/v1/host`.
    pub fn with_host(mut self, host: Arc<dyn HostPort>) -> Self {
        self.host = Some(host);
        self
    }

    /// Serves what the host agent does under `/api/v1/host/agent` and the
    /// paths beside it; without one they say no agent is configured.
    pub fn with_host_agent(mut self, agent: Arc<dyn HostAgentPort>) -> Self {
        self.host_agent = agent;
        self
    }

    /// Serves the container engines the host agent reaches under
    /// `/api/v1/container-engines`; without them those paths say so.
    pub fn with_containers(mut self, containers: Arc<dyn ContainersPort>) -> Self {
        self.containers = containers;
        self
    }

    /// Serves alert rules, channels and notifications under
    /// `/api/v1/alert-*`.
    pub fn with_alerts(mut self, alerts: Arc<dyn AlertsPort>) -> Self {
        self.alerts = Some(alerts);
        self
    }

    /// Serves the certificate inventory under `/api/v1/certificates`.
    pub fn with_certificates(mut self, certificates: Arc<dyn CertificatePort>) -> Self {
        self.certificates = Some(certificates);
        self
    }

    /// Checks HTTPS listeners under `/api/v1/tls-checks`.
    pub fn with_tls_probe(mut self, probe: Arc<dyn TlsProbe>) -> Self {
        self.tls_probe = Some(probe);
        self
    }

    /// Serves sites, upstreams, listeners and the draft under `/api/v1`.
    pub fn with_configuration(mut self, configuration: Arc<dyn ConfigurationPort>) -> Self {
        self.configuration = Some(configuration);
        self
    }

    /// Lists live service instances under `/api/v1/platform/services`.
    pub fn with_directory(mut self, directory: Arc<dyn ServiceDirectory>) -> Self {
        self.directory = Some(directory);
        self
    }

    /// Admits requests by the service's published health: while degraded,
    /// only safe methods are served; while unavailable, nothing is.
    pub fn with_health(mut self, health: HealthWatch) -> Self {
        self.health = Some(health);
        self
    }
}
