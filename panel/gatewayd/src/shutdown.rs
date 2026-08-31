use crate::ShutdownReason;
use panel_errors::{PanelError, Result};
use panel_gateway_runtime::{GatewayEvent, GatewayEventSink, NoopGatewayEventSink};
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
    events: Arc<dyn GatewayEventSink>,
    reason: ShutdownEventReason,
}

#[derive(Clone)]
enum ShutdownEventReason {
    Typed(ShutdownReason),
    Custom(String),
}

impl ShutdownEventReason {
    fn into_string(self) -> String {
        match self {
            Self::Typed(reason) => reason.as_str().to_owned(),
            Self::Custom(reason) => reason,
        }
    }
}

impl ShutdownCoordinator {
    pub fn new(readiness: Arc<dyn ReadinessGate>, policy: ShutdownPolicy) -> Self {
        Self {
            readiness,
            policy,
            events: Arc::new(NoopGatewayEventSink),
            reason: ShutdownEventReason::Typed(ShutdownReason::ShutdownRequested),
        }
    }

    pub fn with_event_sink(mut self, events: Arc<dyn GatewayEventSink>) -> Self {
        self.events = events;
        self
    }

    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = ShutdownEventReason::Custom(reason.into());
        self
    }

    pub fn with_shutdown_reason(mut self, reason: ShutdownReason) -> Self {
        self.reason = ShutdownEventReason::Typed(reason);
        self
    }

    pub async fn run(self, signal: impl Future<Output = ()>) {
        signal.await;
        let reason = self.reason.clone();
        self.complete(reason).await;
    }

    pub async fn run_with_reason(self, signal: impl Future<Output = String>) {
        let reason = signal.await;
        self.complete(ShutdownEventReason::Custom(reason)).await;
    }

    pub async fn run_with_shutdown_reason(self, signal: impl Future<Output = ShutdownReason>) {
        let reason = signal.await;
        self.complete(ShutdownEventReason::Typed(reason)).await;
    }

    async fn complete(self, reason: ShutdownEventReason) {
        self.events.emit(&GatewayEvent::ShutdownStarted {
            drain_millis: u64::try_from(self.policy.drain_timeout().as_millis())
                .unwrap_or(u64::MAX),
            reason: reason.into_string(),
        });
        self.readiness.withdraw().await;
        tokio::time::sleep(self.policy.drain_timeout()).await;
        self.events.emit(&GatewayEvent::ShutdownCompleted);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    };

    struct RecordingReadinessGate(AtomicBool);

    #[tonic::async_trait]
    impl ReadinessGate for RecordingReadinessGate {
        async fn withdraw(&self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[derive(Default)]
    struct RecordingEventSink(Mutex<Vec<GatewayEvent>>);

    impl GatewayEventSink for RecordingEventSink {
        fn emit(&self, event: &GatewayEvent) {
            self.0.lock().unwrap().push(event.clone());
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

    #[tokio::test]
    async fn dynamic_shutdown_reason_preserves_failure_provenance() {
        let readiness = Arc::new(RecordingReadinessGate(AtomicBool::new(false)));
        let events = Arc::new(RecordingEventSink::default());
        let coordinator = ShutdownCoordinator::new(
            Arc::clone(&readiness) as Arc<dyn ReadinessGate>,
            ShutdownPolicy::new(Duration::ZERO).unwrap(),
        )
        .with_event_sink(Arc::clone(&events) as Arc<dyn GatewayEventSink>);

        coordinator
            .run_with_reason(async { "background_task_failure".to_owned() })
            .await;

        assert!(matches!(
            &events.0.lock().unwrap()[0],
            GatewayEvent::ShutdownStarted { reason, .. }
                if reason == "background_task_failure"
        ));
        assert!(readiness.0.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn typed_shutdown_reason_is_converted_only_at_the_event_boundary() {
        let readiness = Arc::new(RecordingReadinessGate(AtomicBool::new(false)));
        let events = Arc::new(RecordingEventSink::default());
        let coordinator = ShutdownCoordinator::new(
            Arc::clone(&readiness) as Arc<dyn ReadinessGate>,
            ShutdownPolicy::new(Duration::ZERO).unwrap(),
        )
        .with_event_sink(Arc::clone(&events) as Arc<dyn GatewayEventSink>);

        coordinator
            .run_with_shutdown_reason(async { ShutdownReason::BackgroundTaskFailure })
            .await;

        assert!(matches!(
            &events.0.lock().unwrap()[0],
            GatewayEvent::ShutdownStarted { reason, .. }
                if reason == ShutdownReason::BackgroundTaskFailure.as_str()
        ));
    }
}
