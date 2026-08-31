use panel_domain::RevisionId;
use panel_errors::ErrorCode;
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::Arc,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum GatewayOperation {
    RestoreActive,
    RestorePrepared,
    SavePrepared,
    CommitActivation,
    DeletePrepared,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum GatewayRequestOperation {
    GetCapabilities,
    Validate,
    Prepare,
    Activate,
    Abort,
    Status,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct GatewayRequestMetadata {
    pub request_id: String,
    pub correlation_id: String,
    pub actor: String,
}

impl GatewayRequestMetadata {
    pub fn new(
        request_id: impl Into<String>,
        correlation_id: impl Into<String>,
        actor: impl Into<String>,
    ) -> Self {
        Self {
            request_id: request_id.into(),
            correlation_id: correlation_id.into(),
            actor: actor.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum GatewayRequestOutcome {
    Succeeded,
    Rejected { error_code: ErrorCode },
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum GatewayEvent {
    RecoveryCompleted {
        ready: bool,
        active_revision_id: Option<RevisionId>,
        prepared_count: usize,
    },
    Prepared {
        revision_id: RevisionId,
        prepared_count: usize,
    },
    Activated {
        revision_id: RevisionId,
        prepared_count: usize,
    },
    Aborted {
        revision_id: RevisionId,
        prepared_count: usize,
    },
    Degraded {
        operation: GatewayOperation,
        error_code: ErrorCode,
    },
    RequestStarted {
        operation: GatewayRequestOperation,
        metadata: GatewayRequestMetadata,
    },
    RequestCompleted {
        operation: GatewayRequestOperation,
        metadata: GatewayRequestMetadata,
        outcome: GatewayRequestOutcome,
        elapsed_micros: u64,
    },
    TransportStarting {
        listen_address: String,
        worker_count: u32,
    },
    ShutdownStarted {
        drain_millis: u64,
        reason: String,
    },
    ShutdownCompleted,
}

/// Driven observability port. Implementations should remain lightweight;
/// buffering and delivery belong to runtime adapters.
pub trait GatewayEventSink: Send + Sync {
    fn emit(&self, event: &GatewayEvent);
}

/// Optional observer kept separate from the panic boundary so metrics remain
/// an adapter concern rather than a requirement of the stable event port.
pub trait GatewayEventPanicObserver: Send + Sync {
    fn event_sink_panicked(&self);
}

/// Universal non-unwinding boundary for injected event sinks.
///
/// This type lives beside the port so every transport, runtime, and composition
/// layer can apply the same safety contract without depending on one another.
#[derive(Clone)]
pub struct PanicIsolatedGatewayEventSink {
    inner: Arc<dyn GatewayEventSink>,
    observer: Option<Arc<dyn GatewayEventPanicObserver>>,
}

impl PanicIsolatedGatewayEventSink {
    pub fn new(inner: Arc<dyn GatewayEventSink>) -> Self {
        Self {
            inner,
            observer: None,
        }
    }

    pub fn with_observer(
        inner: Arc<dyn GatewayEventSink>,
        observer: Arc<dyn GatewayEventPanicObserver>,
    ) -> Self {
        Self {
            inner,
            observer: Some(observer),
        }
    }
}

impl GatewayEventSink for PanicIsolatedGatewayEventSink {
    fn emit(&self, event: &GatewayEvent) {
        if catch_unwind(AssertUnwindSafe(|| self.inner.emit(event))).is_err() {
            if let Some(observer) = &self.observer {
                let _ = catch_unwind(AssertUnwindSafe(|| observer.event_sink_panicked()));
            }
        }
    }
}

#[derive(Default)]
pub struct NoopGatewayEventSink;

impl GatewayEventSink for NoopGatewayEventSink {
    fn emit(&self, _event: &GatewayEvent) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    struct PanickingSink;

    impl GatewayEventSink for PanickingSink {
        fn emit(&self, _event: &GatewayEvent) {
            panic!("injected event panic");
        }
    }

    struct PanickingObserver;

    impl GatewayEventPanicObserver for PanickingObserver {
        fn event_sink_panicked(&self) {
            panic!("injected observer panic");
        }
    }

    #[test]
    fn panic_boundary_also_isolates_a_panicking_observer() {
        let sink = PanicIsolatedGatewayEventSink::with_observer(
            Arc::new(PanickingSink),
            Arc::new(PanickingObserver),
        );

        sink.emit(&GatewayEvent::ShutdownCompleted);
    }
}
