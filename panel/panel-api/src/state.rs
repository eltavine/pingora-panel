use panel_health::HealthWatch;
use std::sync::Arc;

pub struct ApiState<U> {
    pub(crate) use_cases: Arc<U>,
    pub(crate) health: Option<HealthWatch>,
}

impl<U> Clone for ApiState<U> {
    fn clone(&self) -> Self {
        Self {
            use_cases: Arc::clone(&self.use_cases),
            health: self.health.clone(),
        }
    }
}

impl<U> ApiState<U> {
    pub fn new(use_cases: Arc<U>) -> Self {
        Self {
            use_cases,
            health: None,
        }
    }

    /// Admits requests by the service's published health: while degraded,
    /// only safe methods are served; while unavailable, nothing is.
    pub fn with_health(mut self, health: HealthWatch) -> Self {
        self.health = Some(health);
        self
    }
}
