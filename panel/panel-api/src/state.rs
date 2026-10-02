use panel_health::HealthWatch;
use panel_platform::ServiceDirectory;
use std::sync::Arc;

pub struct ApiState<U> {
    pub(crate) use_cases: Arc<U>,
    pub(crate) health: Option<HealthWatch>,
    pub(crate) directory: Option<Arc<dyn ServiceDirectory>>,
}

impl<U> Clone for ApiState<U> {
    fn clone(&self) -> Self {
        Self {
            use_cases: Arc::clone(&self.use_cases),
            health: self.health.clone(),
            directory: self.directory.clone(),
        }
    }
}

impl<U> ApiState<U> {
    pub fn new(use_cases: Arc<U>) -> Self {
        Self {
            use_cases,
            health: None,
            directory: None,
        }
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
