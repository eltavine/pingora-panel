use panel_gateway_runtime::{GatewayEvent, GatewayEventSink, GatewayRequestOutcome};
use tracing_subscriber::EnvFilter;

pub fn initialize_observability() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .try_init();
}

#[derive(Default)]
pub struct TracingGatewayEventSink;

impl GatewayEventSink for TracingGatewayEventSink {
    fn emit(&self, event: &GatewayEvent) {
        match event {
            GatewayEvent::RecoveryCompleted {
                ready,
                active_revision_id,
                prepared_count,
            } => tracing::info!(
                event = "recovery_completed",
                ready,
                active_revision_id = active_revision_id.map(|value| value.get()),
                prepared_count,
                "gateway recovery completed"
            ),
            GatewayEvent::Prepared {
                revision_id,
                prepared_count,
            } => tracing::info!(
                event = "snapshot_prepared",
                revision_id = revision_id.get(),
                prepared_count,
                "snapshot prepared"
            ),
            GatewayEvent::Activated {
                revision_id,
                prepared_count,
            } => tracing::info!(
                event = "snapshot_activated",
                revision_id = revision_id.get(),
                prepared_count,
                "snapshot activated"
            ),
            GatewayEvent::Aborted {
                revision_id,
                prepared_count,
            } => tracing::info!(
                event = "snapshot_aborted",
                revision_id = revision_id.get(),
                prepared_count,
                "prepared snapshot aborted"
            ),
            GatewayEvent::Degraded {
                operation,
                error_code,
            } => tracing::error!(
                event = "gateway_degraded",
                operation = ?operation,
                error_code = %error_code,
                "gateway readiness withdrawn"
            ),
            GatewayEvent::RequestStarted {
                operation,
                metadata,
            } => tracing::info!(
                event = "request_started",
                operation = ?operation,
                request_id = metadata.request_id,
                correlation_id = metadata.correlation_id,
                actor = metadata.actor,
                "gateway request started"
            ),
            GatewayEvent::RequestCompleted {
                operation,
                metadata,
                outcome,
                elapsed_micros,
            } => match outcome {
                GatewayRequestOutcome::Succeeded => tracing::info!(
                    event = "request_completed",
                    operation = ?operation,
                    request_id = metadata.request_id,
                    correlation_id = metadata.correlation_id,
                    actor = metadata.actor,
                    outcome = "succeeded",
                    elapsed_micros,
                    "gateway request completed"
                ),
                GatewayRequestOutcome::Rejected { error_code } => tracing::warn!(
                    event = "request_completed",
                    operation = ?operation,
                    request_id = metadata.request_id,
                    correlation_id = metadata.correlation_id,
                    actor = metadata.actor,
                    outcome = "rejected",
                    error_code = %error_code,
                    elapsed_micros,
                    "gateway request rejected"
                ),
                _ => tracing::debug!(
                    event = "request_completed",
                    operation = ?operation,
                    request_id = metadata.request_id,
                    correlation_id = metadata.correlation_id,
                    actor = metadata.actor,
                    outcome = ?outcome,
                    elapsed_micros,
                    "gateway request completed with an extension outcome"
                ),
            },
            GatewayEvent::TransportStarting {
                listen_address,
                worker_count,
            } => tracing::info!(
                event = "transport_starting",
                listen_address,
                worker_count,
                "gateway transport is starting"
            ),
            GatewayEvent::ShutdownStarted {
                drain_millis,
                reason,
            } => tracing::info!(
                event = "shutdown_started",
                drain_millis,
                reason,
                "gateway shutdown started"
            ),
            GatewayEvent::ShutdownCompleted => tracing::info!(
                event = "shutdown_completed",
                "gateway drain window completed"
            ),
            _ => tracing::debug!(event = ?event, "gateway extension event"),
        }
    }
}
