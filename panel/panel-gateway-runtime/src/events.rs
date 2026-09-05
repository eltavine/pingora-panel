//! Runtime event-delivery adapters around the stable `panel-engine` port.

pub use panel_engine::{
    GatewayEvent, GatewayEventDeliveryDiagnostics, GatewayEventDeliveryDiagnosticsProvider,
    GatewayEventPanicObserver, GatewayEventSink, GatewayOperation, GatewayRequestMetadata,
    GatewayRequestOperation, GatewayRequestOutcome, NoopGatewayEventSink,
    PanicIsolatedGatewayEventSink,
};
use panel_errors::{ErrorCode, PanelError, Result};
use std::{
    future::Future,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use tokio::sync::mpsc;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct GatewayEventDeliverySnapshot {
    queue_full_events: u64,
    disconnected_events: u64,
    consumer_panics: u64,
}

impl GatewayEventDeliverySnapshot {
    pub fn queue_full_events(self) -> u64 {
        self.queue_full_events
    }

    pub fn disconnected_events(self) -> u64 {
        self.disconnected_events
    }

    pub fn dropped_events(self) -> u64 {
        self.queue_full_events
            .saturating_add(self.disconnected_events)
    }

    pub fn consumer_panics(self) -> u64 {
        self.consumer_panics
    }
}

#[derive(Default)]
struct GatewayEventDeliveryStats {
    queue_full_events: AtomicU64,
    disconnected_events: AtomicU64,
    consumer_panics: AtomicU64,
}

#[derive(Clone, Default)]
pub struct GatewayEventDeliveryMonitor {
    stats: Arc<GatewayEventDeliveryStats>,
}

impl GatewayEventDeliveryMonitor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> GatewayEventDeliverySnapshot {
        GatewayEventDeliverySnapshot {
            queue_full_events: self.stats.queue_full_events.load(Ordering::Relaxed),
            disconnected_events: self.stats.disconnected_events.load(Ordering::Relaxed),
            consumer_panics: self.stats.consumer_panics.load(Ordering::Relaxed),
        }
    }

    fn record_queue_full(&self) {
        self.stats.queue_full_events.fetch_add(1, Ordering::Relaxed);
    }

    fn record_disconnected(&self) {
        self.stats
            .disconnected_events
            .fetch_add(1, Ordering::Relaxed);
    }

    fn record_consumer_panic(&self) {
        self.stats.consumer_panics.fetch_add(1, Ordering::Relaxed);
    }
}

impl GatewayEventPanicObserver for GatewayEventDeliveryMonitor {
    fn event_sink_panicked(&self) {
        self.record_consumer_panic();
    }
}

impl GatewayEventDeliveryDiagnosticsProvider for GatewayEventDeliveryMonitor {
    fn snapshot(&self) -> GatewayEventDeliveryDiagnostics {
        let snapshot = GatewayEventDeliveryMonitor::snapshot(self);
        GatewayEventDeliveryDiagnostics::new(
            snapshot.queue_full_events(),
            snapshot.disconnected_events(),
            snapshot.consumer_panics(),
        )
    }
}

/// Compact recovery counters kept independent from any metrics backend.
///
/// Applications can project this provider into Prometheus, OpenTelemetry, or
/// another mature backend without coupling the engine to one telemetry stack.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct GatewayRecoveryDiagnostics {
    recovery_completed: u64,
    degraded_events: u64,
    unknown_commit_outcomes: u64,
}

impl GatewayRecoveryDiagnostics {
    pub fn recovery_completed(self) -> u64 {
        self.recovery_completed
    }

    pub fn degraded_events(self) -> u64 {
        self.degraded_events
    }

    pub fn unknown_commit_outcomes(self) -> u64 {
        self.unknown_commit_outcomes
    }
}

pub trait GatewayRecoveryDiagnosticsProvider: Send + Sync {
    fn recovery_snapshot(&self) -> GatewayRecoveryDiagnostics;
}

#[derive(Default)]
struct GatewayRecoveryStats {
    recovery_completed: AtomicU64,
    degraded_events: AtomicU64,
    unknown_commit_outcomes: AtomicU64,
}

/// Event-driven recovery monitor. It is deliberately a sink so it can be
/// composed through the existing fan-out boundary and replaced by an adapter.
#[derive(Clone, Default)]
pub struct GatewayRecoveryMonitor {
    stats: Arc<GatewayRecoveryStats>,
}

impl GatewayRecoveryMonitor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> GatewayRecoveryDiagnostics {
        GatewayRecoveryDiagnostics {
            recovery_completed: self.stats.recovery_completed.load(Ordering::Relaxed),
            degraded_events: self.stats.degraded_events.load(Ordering::Relaxed),
            unknown_commit_outcomes: self.stats.unknown_commit_outcomes.load(Ordering::Relaxed),
        }
    }
}

impl GatewayRecoveryDiagnosticsProvider for GatewayRecoveryMonitor {
    fn recovery_snapshot(&self) -> GatewayRecoveryDiagnostics {
        self.snapshot()
    }
}

impl GatewayEventSink for GatewayRecoveryMonitor {
    fn emit(&self, event: &GatewayEvent) {
        match event {
            GatewayEvent::RecoveryCompleted { .. } => {
                self.stats.recovery_completed.fetch_add(1, Ordering::Relaxed);
            }
            GatewayEvent::Degraded {
                operation: GatewayOperation::CommitActivation,
                error_code,
            } => {
                self.stats.degraded_events.fetch_add(1, Ordering::Relaxed);
                if error_code.as_str() == ErrorCode::COMMIT_OUTCOME_UNKNOWN {
                    self.stats
                        .unknown_commit_outcomes
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
            GatewayEvent::Degraded { .. } => {
                self.stats.degraded_events.fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
    }
}

/// Panic-isolated fan-out adapter. One extension cannot prevent later consumers
/// from seeing an event or unwind into a gateway transaction.
pub struct FanoutGatewayEventSink {
    sinks: Vec<PanicIsolatedGatewayEventSink>,
    monitor: GatewayEventDeliveryMonitor,
}

impl FanoutGatewayEventSink {
    pub fn new(sinks: impl IntoIterator<Item = Arc<dyn GatewayEventSink>>) -> Self {
        Self::with_monitor(sinks, GatewayEventDeliveryMonitor::new())
    }

    pub fn with_monitor(
        sinks: impl IntoIterator<Item = Arc<dyn GatewayEventSink>>,
        monitor: GatewayEventDeliveryMonitor,
    ) -> Self {
        let observer: Arc<dyn GatewayEventPanicObserver> = Arc::new(monitor.clone());
        Self {
            sinks: sinks
                .into_iter()
                .map(|sink| {
                    PanicIsolatedGatewayEventSink::with_observer(sink, Arc::clone(&observer))
                })
                .collect(),
            monitor,
        }
    }

    pub fn consumer_panics(&self) -> u64 {
        self.monitor.snapshot().consumer_panics()
    }

    pub fn delivery_monitor(&self) -> GatewayEventDeliveryMonitor {
        self.monitor.clone()
    }
}

impl GatewayEventSink for FanoutGatewayEventSink {
    fn emit(&self, event: &GatewayEvent) {
        for sink in &self.sinks {
            sink.emit(event);
        }
    }
}

pub struct BufferedGatewayEventSink {
    sender: mpsc::Sender<GatewayEvent>,
    monitor: GatewayEventDeliveryMonitor,
}

impl BufferedGatewayEventSink {
    pub fn channel(capacity: usize) -> Result<(Arc<Self>, BufferedGatewayEventReceiver)> {
        Self::channel_with_monitor(capacity, GatewayEventDeliveryMonitor::new())
    }

    pub fn channel_with_monitor(
        capacity: usize,
        monitor: GatewayEventDeliveryMonitor,
    ) -> Result<(Arc<Self>, BufferedGatewayEventReceiver)> {
        if capacity == 0 {
            return Err(PanelError::invalid_argument(
                "gateway event buffer capacity must be non-zero",
            ));
        }
        let (sender, receiver) = mpsc::channel(capacity);
        Ok((
            Arc::new(Self {
                sender,
                monitor: monitor.clone(),
            }),
            BufferedGatewayEventReceiver { receiver, monitor },
        ))
    }

    pub fn dropped_events(&self) -> u64 {
        self.monitor.snapshot().dropped_events()
    }

    pub fn consumer_panics(&self) -> u64 {
        self.monitor.snapshot().consumer_panics()
    }

    pub fn delivery_monitor(&self) -> GatewayEventDeliveryMonitor {
        self.monitor.clone()
    }
}

impl GatewayEventSink for BufferedGatewayEventSink {
    fn emit(&self, event: &GatewayEvent) {
        match self.sender.try_send(event.clone()) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => self.monitor.record_queue_full(),
            Err(mpsc::error::TrySendError::Closed(_)) => self.monitor.record_disconnected(),
        }
    }
}

pub struct BufferedGatewayEventReceiver {
    receiver: mpsc::Receiver<GatewayEvent>,
    monitor: GatewayEventDeliveryMonitor,
}

impl BufferedGatewayEventReceiver {
    pub async fn run(mut self, downstream: Arc<dyn GatewayEventSink>) {
        let downstream = self.isolate(downstream);
        while let Some(event) = self.receiver.recv().await {
            self.deliver(downstream.as_ref(), &event);
        }
    }

    pub async fn run_until_shutdown(
        mut self,
        downstream: Arc<dyn GatewayEventSink>,
        shutdown: impl Future<Output = ()>,
    ) {
        let downstream = self.isolate(downstream);
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                _ = &mut shutdown => {
                    self.receiver.close();
                    while let Some(event) = self.receiver.recv().await {
                        self.deliver(downstream.as_ref(), &event);
                    }
                    return;
                }
                event = self.receiver.recv() => match event {
                    Some(event) => self.deliver(downstream.as_ref(), &event),
                    None => return,
                }
            }
        }
    }

    fn isolate(&self, downstream: Arc<dyn GatewayEventSink>) -> Arc<dyn GatewayEventSink> {
        Arc::new(PanicIsolatedGatewayEventSink::with_observer(
            downstream,
            Arc::new(self.monitor.clone()),
        ))
    }

    fn deliver(&self, downstream: &dyn GatewayEventSink, event: &GatewayEvent) {
        downstream.emit(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingSink(Mutex<Vec<GatewayEvent>>);

    impl GatewayEventSink for RecordingSink {
        fn emit(&self, event: &GatewayEvent) {
            self.0.lock().unwrap().push(event.clone());
        }
    }

    struct PanickingSink;

    impl GatewayEventSink for PanickingSink {
        fn emit(&self, _event: &GatewayEvent) {
            panic!("injected event consumer panic");
        }
    }

    fn event() -> GatewayEvent {
        GatewayEvent::ShutdownCompleted
    }

    #[test]
    fn recovery_monitor_projects_stable_counters() {
        let monitor = GatewayRecoveryMonitor::new();
        monitor.emit(&GatewayEvent::RecoveryCompleted {
            ready: true,
            active_revision_id: None,
            prepared_count: 0,
        });
        monitor.emit(&GatewayEvent::Degraded {
            operation: GatewayOperation::CommitActivation,
            error_code: ErrorCode::COMMIT_OUTCOME_UNKNOWN.into(),
        });
        monitor.emit(&GatewayEvent::Degraded {
            operation: GatewayOperation::RestorePrepared,
            error_code: ErrorCode::CORRUPT_STATE.into(),
        });

        let snapshot = monitor.snapshot();
        assert_eq!(snapshot.recovery_completed(), 1);
        assert_eq!(snapshot.degraded_events(), 2);
        assert_eq!(snapshot.unknown_commit_outcomes(), 1);
        assert_eq!(
            GatewayRecoveryDiagnosticsProvider::recovery_snapshot(&monitor),
            snapshot
        );
    }

    #[test]
    fn fanout_isolates_a_panicking_extension() {
        let recording = Arc::new(RecordingSink::default());
        let fanout = FanoutGatewayEventSink::new([
            Arc::new(PanickingSink) as Arc<dyn GatewayEventSink>,
            Arc::clone(&recording) as Arc<dyn GatewayEventSink>,
        ]);

        fanout.emit(&event());

        assert_eq!(fanout.consumer_panics(), 1);
        assert_eq!(recording.0.lock().unwrap().as_slice(), &[event()]);
    }

    #[test]
    fn decorator_isolates_a_directly_injected_sink() {
        let monitor = GatewayEventDeliveryMonitor::new();
        let sink = PanicIsolatedGatewayEventSink::with_observer(
            Arc::new(PanickingSink),
            Arc::new(monitor.clone()),
        );

        sink.emit(&event());

        assert_eq!(monitor.snapshot().consumer_panics(), 1);
    }

    #[tokio::test]
    async fn bounded_buffer_drops_instead_of_blocking_the_producer() {
        let (sink, receiver) = BufferedGatewayEventSink::channel(1).unwrap();
        sink.emit(&event());
        sink.emit(&event());
        assert_eq!(sink.dropped_events(), 1);
        assert_eq!(sink.delivery_monitor().snapshot().queue_full_events(), 1);

        let recording = Arc::new(RecordingSink::default());
        drop(sink);
        receiver
            .run(Arc::clone(&recording) as Arc<dyn GatewayEventSink>)
            .await;
        assert_eq!(recording.0.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn cooperative_shutdown_closes_and_drains_the_queue() {
        let (sink, receiver) = BufferedGatewayEventSink::channel(4).unwrap();
        sink.emit(&event());
        sink.emit(&event());
        let recording = Arc::new(RecordingSink::default());

        receiver
            .run_until_shutdown(Arc::clone(&recording) as Arc<dyn GatewayEventSink>, async {
            })
            .await;

        assert_eq!(recording.0.lock().unwrap().len(), 2);
        sink.emit(&event());
        assert_eq!(sink.delivery_monitor().snapshot().disconnected_events(), 1);
    }

    #[test]
    fn diagnostics_provider_projects_the_monitor_without_exposing_queue_types() {
        let monitor = GatewayEventDeliveryMonitor::new();
        monitor.record_queue_full();
        monitor.record_disconnected();
        monitor.record_consumer_panic();

        let diagnostics = GatewayEventDeliveryDiagnosticsProvider::snapshot(&monitor);
        assert_eq!(diagnostics.queue_full_events(), 1);
        assert_eq!(diagnostics.disconnected_events(), 1);
        assert_eq!(diagnostics.dropped_events(), 2);
        assert_eq!(diagnostics.consumer_panics(), 1);
    }
}
