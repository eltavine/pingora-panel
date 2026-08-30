use panel_gateway_runtime::{GatewayEvent, GatewayEventSink};
use tokio::sync::watch;
use tonic_health::{server::HealthReporter, ServingStatus};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeReadiness {
    Starting,
    Ready,
    NotReady,
}

pub struct RuntimeHealthState {
    sender: watch::Sender<RuntimeReadiness>,
}

impl RuntimeHealthState {
    pub fn new() -> Self {
        let (sender, _) = watch::channel(RuntimeReadiness::Starting);
        Self { sender }
    }

    pub fn current(&self) -> RuntimeReadiness {
        *self.sender.borrow()
    }

    pub fn subscribe(&self) -> watch::Receiver<RuntimeReadiness> {
        self.sender.subscribe()
    }
}

impl Default for RuntimeHealthState {
    fn default() -> Self {
        Self::new()
    }
}

impl GatewayEventSink for RuntimeHealthState {
    fn emit(&self, event: &GatewayEvent) {
        let next = match event {
            GatewayEvent::RecoveryCompleted { ready: true, .. } => Some(RuntimeReadiness::Ready),
            GatewayEvent::RecoveryCompleted { ready: false, .. }
            | GatewayEvent::Degraded { .. } => Some(RuntimeReadiness::NotReady),
            _ => None,
        };
        if let Some(next) = next {
            self.sender.send_replace(next);
        }
    }
}

pub struct TonicHealthSynchronizer {
    receiver: watch::Receiver<RuntimeReadiness>,
    reporter: HealthReporter,
    service_names: Vec<String>,
}

impl TonicHealthSynchronizer {
    pub fn new(
        receiver: watch::Receiver<RuntimeReadiness>,
        reporter: HealthReporter,
        service_names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            receiver,
            reporter,
            service_names: service_names.into_iter().map(Into::into).collect(),
        }
    }

    pub async fn run(mut self) {
        let initial = *self.receiver.borrow();
        self.publish(initial).await;
        while self.receiver.changed().await.is_ok() {
            let readiness = *self.receiver.borrow_and_update();
            self.publish(readiness).await;
        }
    }

    async fn publish(&self, readiness: RuntimeReadiness) {
        let serving = match readiness {
            RuntimeReadiness::Ready => ServingStatus::Serving,
            RuntimeReadiness::Starting | RuntimeReadiness::NotReady => ServingStatus::NotServing,
        };
        for service_name in &self.service_names {
            self.reporter
                .set_service_status(service_name, serving)
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_errors::ErrorCode;
    use panel_gateway_runtime::GatewayOperation;
    use tonic_health::pb::{
        health_check_response::ServingStatus, health_server::Health, HealthCheckRequest,
    };

    #[tokio::test]
    async fn degraded_runtime_event_withdraws_tonic_readiness() {
        let state = RuntimeHealthState::new();
        let reporter = HealthReporter::new();
        let service = tonic_health::server::HealthService::from_health_reporter(reporter.clone());
        let synchronizer = TonicHealthSynchronizer::new(state.subscribe(), reporter, ["gateway"]);
        let task = tokio::spawn(synchronizer.run());

        state.emit(&GatewayEvent::RecoveryCompleted {
            ready: true,
            active_revision_id: None,
            prepared_count: 0,
        });
        tokio::task::yield_now().await;
        let ready = service
            .check(tonic::Request::new(HealthCheckRequest {
                service: "gateway".into(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(ready.status, ServingStatus::Serving as i32);

        state.emit(&GatewayEvent::Degraded {
            operation: GatewayOperation::SavePrepared,
            error_code: ErrorCode::from(ErrorCode::STORAGE_UNAVAILABLE),
        });
        tokio::task::yield_now().await;
        let degraded = service
            .check(tonic::Request::new(HealthCheckRequest {
                service: "gateway".into(),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(degraded.status, ServingStatus::NotServing as i32);

        task.abort();
    }
}
