use panel_application::{ConfigurationPort, GatewayRuntimePort};
use panel_health::HealthWatch;
use panel_platform::ServiceDirectory;
use std::sync::Arc;

pub struct ApiState<U> {
    pub(crate) use_cases: Arc<U>,
    pub(crate) health: Option<HealthWatch>,
    pub(crate) directory: Option<Arc<dyn ServiceDirectory>>,
    pub(crate) configuration: Option<Arc<dyn ConfigurationPort>>,
    pub(crate) runtime: Option<Arc<dyn GatewayRuntimePort>>,
}

impl<U> Clone for ApiState<U> {
    fn clone(&self) -> Self {
        Self {
            use_cases: Arc::clone(&self.use_cases),
            health: self.health.clone(),
            directory: self.directory.clone(),
            configuration: self.configuration.clone(),
            runtime: self.runtime.clone(),
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
        }
    }

    /// Serves data plane status, reloads, workers, shutdown and upstream health.
    pub fn with_runtime(mut self, runtime: Arc<dyn GatewayRuntimePort>) -> Self {
        self.runtime = Some(runtime);
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
