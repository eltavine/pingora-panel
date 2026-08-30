use panel_errors::{PanelError, Result};
use std::{future::Future, sync::Arc, time::Duration};
use tonic_health::{server::HealthReporter, ServingStatus};

pub const DEFAULT_DRAIN_TIMEOUT: Duration = Duration::from_secs(1);
pub const MAX_DRAIN_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ShutdownPolicy {
    drain_timeout: Duration,
}

impl ShutdownPolicy {
    pub fn new(drain_timeout: Duration) -> Result<Self> {
        if drain_timeout > MAX_DRAIN_TIMEOUT {
            return Err(PanelError::invalid_argument(format!(
                "gateway drain timeout must not exceed {} milliseconds",
                MAX_DRAIN_TIMEOUT.as_millis()
            )));
        }
        Ok(Self { drain_timeout })
    }

    pub fn drain_timeout(self) -> Duration {
        self.drain_timeout
    }
}

impl Default for ShutdownPolicy {
    fn default() -> Self {
        Self {
            drain_timeout: DEFAULT_DRAIN_TIMEOUT,
        }
    }
}

#[tonic::async_trait]
pub trait ReadinessGate: Send + Sync {
    async fn withdraw(&self);
}

pub struct TonicHealthReadinessGate {
    health_reporter: HealthReporter,
    service_names: Vec<String>,
}

impl TonicHealthReadinessGate {
    pub fn new(
        health_reporter: HealthReporter,
        service_names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            health_reporter,
            service_names: service_names.into_iter().map(Into::into).collect(),
        }
    }
}

#[tonic::async_trait]
impl ReadinessGate for TonicHealthReadinessGate {
    async fn withdraw(&self) {
        for service_name in &self.service_names {
            self.health_reporter
                .set_service_status(service_name, ServingStatus::NotServing)
                .await;
        }
    }
}

/// Coordinates readiness withdrawal before transport shutdown.
///
/// Signal acquisition and health protocol details stay in adapters. This type only
/// sequences readiness withdrawal, the drain window, and transport termination.
pub struct ShutdownCoordinator {
    readiness: Arc<dyn ReadinessGate>,
    policy: ShutdownPolicy,
}

impl ShutdownCoordinator {
    pub fn new(readiness: Arc<dyn ReadinessGate>, policy: ShutdownPolicy) -> Self {
        Self { readiness, policy }
    }

    pub async fn run(self, signal: impl Future<Output = ()>) {
        signal.await;
        self.readiness.withdraw().await;
        tokio::time::sleep(self.policy.drain_timeout()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct RecordingReadinessGate(AtomicBool);

    #[tonic::async_trait]
    impl ReadinessGate for RecordingReadinessGate {
        async fn withdraw(&self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn drain_timeout_is_bounded() {
        assert!(ShutdownPolicy::new(Duration::ZERO).is_ok());
        assert!(ShutdownPolicy::new(MAX_DRAIN_TIMEOUT).is_ok());
        let error = ShutdownPolicy::new(MAX_DRAIN_TIMEOUT + Duration::from_millis(1)).unwrap_err();
        assert_eq!(
            error.code.as_str(),
            panel_errors::ErrorCode::INVALID_ARGUMENT
        );
    }

    #[tokio::test]
    async fn coordinator_withdraws_readiness_before_returning() {
        let readiness = Arc::new(RecordingReadinessGate(AtomicBool::new(false)));
        let coordinator = ShutdownCoordinator::new(
            Arc::clone(&readiness) as Arc<dyn ReadinessGate>,
            ShutdownPolicy::new(Duration::ZERO).unwrap(),
        );

        coordinator.run(async {}).await;

        assert!(readiness.0.load(Ordering::SeqCst));
    }
}
