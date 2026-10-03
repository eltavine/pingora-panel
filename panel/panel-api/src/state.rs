use crate::{
    access::{AccessAudit, AccessSettings, Gate},
    sign_in::ProviderAccess,
};
use panel_application::{
    AuditPort, CertificatePort, ConfigurationPort, GatewayRuntimePort, TlsProbe,
};
use panel_health::HealthWatch;
use panel_identity::{Identity, ProviderDirectory, ProviderSignIns};
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
    pub(crate) identity: Option<Arc<Gate>>,
    pub(crate) providers: Option<Arc<ProviderAccess>>,
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
            identity: self.identity.clone(),
            providers: self.providers.clone(),
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
            identity: None,
            providers: None,
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
