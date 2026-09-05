#![forbid(unsafe_code)]

//! `gatewayd` composition factory.
//!
//! The binary owns executor startup and OS signal selection. This library owns
//! configuration parsing and concrete adapter wiring so production and black-box
//! tests use the same dependency graph.

mod background_tasks;
mod bind_policy;
mod config;
mod failure_latch;
mod health;
mod observability;
mod resource_limits;
mod runtime_info;
mod shutdown;
mod shutdown_trigger;

pub use background_tasks::{
    BackgroundTaskError, BackgroundTaskFailure, BackgroundTaskFailureKind,
    BackgroundTaskFailureMonitor, BackgroundTaskShutdown, BackgroundTaskShutdownPolicy,
    BackgroundTaskSupervisor, DEFAULT_BACKGROUND_TASK_SHUTDOWN_TIMEOUT,
    MAX_BACKGROUND_TASK_SHUTDOWN_TIMEOUT,
};
pub use bind_policy::{LoopbackOnlyManagementBindPolicy, ManagementBindPolicy};
pub use config::{
    GatewayWorkerCount, GatewaydConfig, BACKGROUND_TASK_SHUTDOWN_TIMEOUT_MILLIS_ENV,
    DRAIN_TIMEOUT_MILLIS_ENV, GATEWAY_ADDRESS_ENV, MAX_GATEWAY_WORKERS, STATE_DIRECTORY_ENV,
    WORKER_COUNT_ENV,
};
pub use health::{RuntimeHealthState, RuntimeReadiness, TonicHealthSynchronizer};
pub use observability::{initialize_observability, TracingGatewayEventSink};
pub use resource_limits::*;
pub use runtime_info::ProcessRuntimeInfo;
pub use shutdown::{ReadinessGate, ShutdownCoordinator, ShutdownPolicy, TonicHealthReadinessGate};
pub use shutdown_trigger::{ShutdownArbiter, ShutdownReason, ShutdownTrigger};

use gateway_grpc::{
    DeadlineRequirement, GatewayGrpcService, GatewayRequestMetadataLimits, GatewayRequestPolicy,
    GatewayTransportPolicy, StandardGatewayRequestPolicy,
};
use gateway_pingora::PingoraGatewayAdapter;
use panel_contracts::gateway::v1::gateway_engine_server::GatewayEngineServer;
use panel_engine::GatewayRuntimeInfoProvider;
use panel_errors::Result;
use panel_gateway_runtime::{
    BufferedGatewayEventSink, DurableGatewayEngine, DurableGatewayEngineOptions,
    FanoutGatewayEventSink, GatewayEvent, GatewayEventDeliveryMonitor, GatewayEventSink,
    GatewayMutationExecutor, GatewayRecoveryMonitor, PreparedSnapshotAdmissionPolicy,
    PreparedSnapshotBudget,
};
use snapshot_store_fs::FileSnapshotStore;
use std::{future::Future, num::NonZeroU32, path::PathBuf, sync::Arc};
use tokio::sync::oneshot;
use tonic::server::NamedService;
use tonic::transport::Server;
use tonic_health::{
    pb::health_server::HealthServer,
    server::{HealthReporter, HealthService},
    ServingStatus,
};

pub type GatewaydEngine = DurableGatewayEngine<PingoraGatewayAdapter, FileSnapshotStore>;
pub type GatewaydTransport = GatewayGrpcService<GatewaydEngine>;
pub type GatewaydHealth = HealthServer<HealthService>;

pub struct GatewaydServices {
    pub gateway: GatewaydTransport,
    pub health: GatewaydHealth,
    pub health_reporter: HealthReporter,
}

#[non_exhaustive]
pub struct GatewaydRuntime {
    pub services: GatewaydServices,
    pub background_tasks: BackgroundTaskSupervisor,
    pub events: Arc<dyn GatewayEventSink>,
    pub event_delivery: GatewayEventDeliveryMonitor,
    pub recovery: GatewayRecoveryMonitor,
    pub mutations: GatewayMutationExecutor,
}

pub struct GatewaydServiceOptions {
    transport_policy: GatewayTransportPolicy,
    prepared_policy: Arc<dyn PreparedSnapshotAdmissionPolicy>,
    request_policy: Option<Arc<dyn GatewayRequestPolicy>>,
    request_metadata_limits: GatewayRequestMetadataLimits,
    event_sinks: Vec<Arc<dyn GatewayEventSink>>,
    event_buffer_capacity: usize,
}

impl GatewaydServiceOptions {
    pub fn with_transport_policy(mut self, transport_policy: GatewayTransportPolicy) -> Self {
        self.transport_policy = transport_policy;
        self
    }

    pub fn with_prepared_policy(
        mut self,
        prepared_policy: Arc<dyn PreparedSnapshotAdmissionPolicy>,
    ) -> Self {
        self.prepared_policy = prepared_policy;
        self
    }

    pub fn with_request_policy(mut self, request_policy: Arc<dyn GatewayRequestPolicy>) -> Self {
        self.request_policy = Some(request_policy);
        self
    }

    pub fn with_request_metadata_limits(
        mut self,
        request_metadata_limits: GatewayRequestMetadataLimits,
    ) -> Self {
        self.request_metadata_limits = request_metadata_limits;
        self
    }

    pub fn with_event_sink(mut self, event_sink: Arc<dyn GatewayEventSink>) -> Self {
        self.event_sinks.push(event_sink);
        self
    }

    pub fn with_event_buffer_capacity(mut self, capacity: usize) -> Self {
        self.event_buffer_capacity = capacity;
        self
    }
}

impl Default for GatewaydServiceOptions {
    fn default() -> Self {
        Self {
            transport_policy: GatewayTransportPolicy::default(),
            prepared_policy: Arc::new(PreparedSnapshotBudget::default()),
            request_policy: None,
            request_metadata_limits: GatewayRequestMetadataLimits::default(),
            event_sinks: Vec::new(),
            event_buffer_capacity: DEFAULT_EVENT_BUFFER_CAPACITY,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GatewaydError {
    #[error("gateway configuration or composition failed: {0}")]
    Panel(#[from] panel_errors::PanelError),
    #[error("gateway transport failed: {0}")]
    Transport(#[from] tonic::transport::Error),
    #[error("gateway executor failed: {0}")]
    Executor(#[from] std::io::Error),
    #[error("gateway background task failed: {0}")]
    BackgroundTask(#[from] BackgroundTaskError),
}

pub fn gateway_service_name() -> &'static str {
    <GatewayEngineServer<GatewaydTransport> as NamedService>::NAME
}

pub async fn build_gateway_services(
    state_directory: impl Into<PathBuf>,
) -> Result<GatewaydServices> {
    let runtime_info = Arc::new(ProcessRuntimeInfo::new(
        env!("CARGO_PKG_VERSION"),
        NonZeroU32::MIN,
    ));
    build_gateway_services_with_runtime_info(state_directory, runtime_info).await
}

pub async fn build_gateway_services_with_runtime_info(
    state_directory: impl Into<PathBuf>,
    runtime_info: Arc<dyn GatewayRuntimeInfoProvider>,
) -> Result<GatewaydServices> {
    build_gateway_services_with_options(
        state_directory,
        runtime_info,
        GatewaydServiceOptions::default(),
    )
    .await
}

pub async fn build_gateway_services_with_options(
    state_directory: impl Into<PathBuf>,
    runtime_info: Arc<dyn GatewayRuntimeInfoProvider>,
    options: GatewaydServiceOptions,
) -> Result<GatewaydServices> {
    Ok(
        build_gateway_runtime_with_options(state_directory, runtime_info, options)
            .await?
            .services,
    )
}

pub async fn build_gateway_runtime_with_options(
    state_directory: impl Into<PathBuf>,
    runtime_info: Arc<dyn GatewayRuntimeInfoProvider>,
    options: GatewaydServiceOptions,
) -> Result<GatewaydRuntime> {
    if options.event_buffer_capacity == 0 {
        return Err(panel_errors::PanelError::invalid_argument(
            "gateway event buffer capacity must be non-zero",
        ));
    }
    let background_tasks = BackgroundTaskSupervisor::new();
    let mutations = GatewayMutationExecutor::new();
    let mutations_for_shutdown = mutations.clone();
    background_tasks.spawn_cooperative_critical(
        "gateway-mutation-drain",
        move |shutdown| async move {
            shutdown.requested().await;
            mutations_for_shutdown.close();
            mutations_for_shutdown.wait().await;
        },
    );
    let adapter = Arc::new(PingoraGatewayAdapter::new());
    let store = Arc::new(FileSnapshotStore::open_exclusive(state_directory).await?);
    let health_state = Arc::new(RuntimeHealthState::new());
    let event_delivery = GatewayEventDeliveryMonitor::new();
    let recovery = GatewayRecoveryMonitor::new();
    let mut direct_sinks = vec![Arc::clone(&health_state) as Arc<dyn GatewayEventSink>];
    direct_sinks.push(Arc::new(recovery.clone()) as Arc<dyn GatewayEventSink>);
    if !options.event_sinks.is_empty() {
        let downstream: Arc<dyn GatewayEventSink> = Arc::new(FanoutGatewayEventSink::with_monitor(
            options.event_sinks,
            event_delivery.clone(),
        ));
        let (buffered_sink, receiver) = BufferedGatewayEventSink::channel_with_monitor(
            options.event_buffer_capacity,
            event_delivery.clone(),
        )?;
        background_tasks.spawn_cooperative_critical("gateway-event-dispatch", move |shutdown| {
            receiver.run_until_shutdown(downstream, shutdown.requested())
        });
        direct_sinks.push(buffered_sink as Arc<dyn GatewayEventSink>);
    }
    let events: Arc<dyn GatewayEventSink> = Arc::new(FanoutGatewayEventSink::with_monitor(
        direct_sinks,
        event_delivery.clone(),
    ));
    let engine = Arc::new(
        DurableGatewayEngine::restore_with_options(
            adapter,
            store,
            DurableGatewayEngineOptions::default()
                .with_prepared_policy(options.prepared_policy)
                .with_event_sink(Arc::clone(&events))
                .with_mutation_executor(mutations.clone()),
        )
        .await?,
    );
    let serving_status = match health_state.current() {
        RuntimeReadiness::Ready => ServingStatus::Serving,
        RuntimeReadiness::Starting | RuntimeReadiness::NotReady => ServingStatus::NotServing,
    };

    let health_reporter = HealthReporter::new();
    health_reporter.set_service_status("", serving_status).await;
    health_reporter
        .set_service_status(gateway_service_name(), serving_status)
        .await;
    let health = HealthServer::new(HealthService::from_health_reporter(health_reporter.clone()));
    let health_sync = TonicHealthSynchronizer::new(
        health_state.subscribe(),
        health_reporter.clone(),
        ["", gateway_service_name()],
    );
    background_tasks.spawn_critical("tonic-health-synchronizer", health_sync.run());

    let request_metadata_limits = options.request_metadata_limits;
    let request_policy = options.request_policy.unwrap_or_else(|| {
        Arc::new(
            StandardGatewayRequestPolicy::with_metadata_limits(
                options.transport_policy.request_timeout(),
                DeadlineRequirement::Optional,
                request_metadata_limits,
            )
            .expect("default transport policy has a non-zero timeout"),
        )
    });

    let task_lifetime: Arc<dyn Send + Sync> = Arc::new(background_tasks.clone());
    Ok(GatewaydRuntime {
        services: GatewaydServices {
            gateway: GatewayGrpcService::with_dependencies(
                engine,
                Some(runtime_info),
                request_policy,
                options.transport_policy,
            )
            .with_event_sink(Arc::clone(&events))
            .with_event_metadata_limits(request_metadata_limits)
            .with_event_delivery_diagnostics(Arc::new(event_delivery.clone()))
            .with_recovery_diagnostics(Arc::new(recovery.clone()))
            .with_lifetime_dependency(task_lifetime),
            health,
            health_reporter,
        },
        background_tasks,
        events,
        event_delivery,
        recovery,
        mutations,
    })
}

pub async fn serve_gatewayd(
    config: GatewaydConfig,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::result::Result<(), GatewaydError> {
    let runtime_info = Arc::new(ProcessRuntimeInfo::new(
        env!("CARGO_PKG_VERSION"),
        config.worker_count().as_non_zero(),
    ));
    let resource_limits = config.resource_limits();
    let tracing_events: Arc<dyn GatewayEventSink> = Arc::new(TracingGatewayEventSink);
    let request_policy = Arc::new(StandardGatewayRequestPolicy::with_metadata_limits(
        resource_limits.transport_policy().request_timeout(),
        resource_limits.deadline_requirement(),
        resource_limits.request_metadata_limits(),
    )?);
    let runtime = build_gateway_runtime_with_options(
        config.state_directory().to_path_buf(),
        runtime_info,
        GatewaydServiceOptions::default()
            .with_transport_policy(resource_limits.transport_policy())
            .with_prepared_policy(Arc::new(resource_limits.prepared_snapshot_budget()))
            .with_request_policy(request_policy)
            .with_request_metadata_limits(resource_limits.request_metadata_limits())
            .with_event_buffer_capacity(resource_limits.event_buffer_capacity())
            .with_event_sink(tracing_events),
    )
    .await?;
    let GatewaydRuntime {
        services,
        background_tasks,
        events,
        event_delivery,
        recovery,
        mutations: _,
    } = runtime;
    let mut failure_monitor = background_tasks.failure_monitor();
    let (trigger_sender, trigger_receiver) = oneshot::channel();
    let shutdown_reason = async move {
        let trigger = ShutdownArbiter::wait(shutdown, failure_monitor.next()).await;
        let reason = trigger.reason();
        let _ = trigger_sender.send(trigger);
        reason
    };
    let readiness = Arc::new(TonicHealthReadinessGate::new(
        services.health_reporter.clone(),
        ["", gateway_service_name()],
    ));
    let shutdown_coordinator = ShutdownCoordinator::new(readiness, config.shutdown_policy())
        .with_event_sink(Arc::clone(&events));
    events.emit(&GatewayEvent::TransportStarting {
        listen_address: config.listen_address().to_string(),
        worker_count: config.worker_count().get(),
    });
    let transport_policy = services.gateway.transport_policy();

    let transport_result = Server::builder()
        .concurrency_limit_per_connection(transport_policy.max_concurrent_requests())
        .timeout(transport_policy.request_timeout())
        .add_service(transport_policy.gateway_server(services.gateway))
        .add_service(services.health)
        .serve_with_shutdown(
            config.listen_address(),
            shutdown_coordinator.run_with_shutdown_reason(shutdown_reason),
        )
        .await;
    let background_result = background_tasks
        .shutdown_and_join_with_policy(config.background_task_shutdown_policy())
        .await;
    let delivery = event_delivery.snapshot();
    let recovery_snapshot = recovery.snapshot();
    tracing::info!(
        event = "event_delivery_stopped",
        queue_full_events = delivery.queue_full_events(),
        disconnected_events = delivery.disconnected_events(),
        consumer_panics = delivery.consumer_panics(),
        "gateway event delivery stopped"
    );
    tracing::info!(
        event = "gateway_recovery_summary",
        recovery_completed = recovery_snapshot.recovery_completed(),
        degraded_events = recovery_snapshot.degraded_events(),
        unknown_commit_outcomes = recovery_snapshot.unknown_commit_outcomes(),
        "gateway recovery summary"
    );
    if let Ok(Some(failure)) = trigger_receiver
        .await
        .map(ShutdownTrigger::into_background_failure)
    {
        return Err(BackgroundTaskError::from_failure(failure).into());
    }
    background_result?;
    transport_result?;
    Ok(())
}

pub async fn build_gateway_transport(
    state_directory: impl Into<PathBuf>,
) -> Result<GatewaydTransport> {
    Ok(build_gateway_services(state_directory).await?.gateway)
}

#[cfg(test)]
mod composition_tests {
    use super::*;
    use panel_contracts::{
        common::v1 as common,
        gateway::v1::{self as wire, gateway_engine_server::GatewayEngine as GatewayEngineService},
    };
    use panel_engine::NoopGatewayEventSink;
    use std::fs;
    use tonic::Request;
    use uuid::Uuid;

    struct TemporaryDirectory(PathBuf);

    impl TemporaryDirectory {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("pingora-panel-gatewayd-{}", Uuid::new_v4())))
        }
    }

    impl Drop for TemporaryDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn runtime_info() -> Arc<dyn GatewayRuntimeInfoProvider> {
        Arc::new(ProcessRuntimeInfo::new("test", NonZeroU32::MIN))
    }

    #[tokio::test]
    async fn composition_registers_every_background_adapter() {
        let temporary = TemporaryDirectory::new();
        let runtime = build_gateway_runtime_with_options(
            &temporary.0,
            runtime_info(),
            GatewaydServiceOptions::default().with_event_sink(Arc::new(NoopGatewayEventSink)),
        )
        .await
        .unwrap();

        assert_eq!(runtime.background_tasks.task_count(), 3);
        assert_eq!(runtime.mutations.pending_tasks(), 0);
        let status = runtime
            .services
            .gateway
            .status(Request::new(wire::StatusRequest {
                context: Some(common::RequestContext {
                    request_id: "status".into(),
                    correlation_id: "composition".into(),
                    actor: "test".into(),
                    deadline: String::new(),
                    idempotency_key: String::new(),
                    schema_version: panel_contracts::PROTOCOL_VERSION.into(),
                }),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(status.recovery.unwrap().recovery_completed, 1);
        runtime.background_tasks.shutdown_and_join().await.unwrap();
        assert_eq!(runtime.background_tasks.task_count(), 0);
        let delivery = runtime.event_delivery.snapshot();
        assert_eq!(delivery.queue_full_events(), 0);
        assert_eq!(delivery.disconnected_events(), 0);
        assert_eq!(delivery.consumer_panics(), 0);
    }

    #[tokio::test]
    async fn invalid_event_capacity_fails_before_touching_state() {
        let temporary = TemporaryDirectory::new();
        let error = build_gateway_runtime_with_options(
            &temporary.0,
            runtime_info(),
            GatewaydServiceOptions::default().with_event_buffer_capacity(0),
        )
        .await
        .err()
        .unwrap();

        assert_eq!(
            error.code.as_str(),
            panel_errors::ErrorCode::INVALID_ARGUMENT
        );
        assert!(!temporary.0.exists());
    }

    #[tokio::test]
    async fn composition_applies_injected_request_metadata_limits() {
        let temporary = TemporaryDirectory::new();
        let limits = GatewayRequestMetadataLimits::new(4, 64, 64, 64, 64, 64).unwrap();
        let runtime = build_gateway_runtime_with_options(
            &temporary.0,
            runtime_info(),
            GatewaydServiceOptions::default().with_request_metadata_limits(limits),
        )
        .await
        .unwrap();

        let response = runtime
            .services
            .gateway
            .get_capabilities(Request::new(wire::GetCapabilitiesRequest {
                context: Some(common::RequestContext {
                    request_id: "12345".into(),
                    correlation_id: "test".into(),
                    actor: "test".into(),
                    deadline: String::new(),
                    idempotency_key: String::new(),
                    schema_version: panel_contracts::PROTOCOL_VERSION.into(),
                }),
            }))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(
            response.error.unwrap().code,
            panel_errors::ErrorCode::RESOURCE_EXHAUSTED
        );
        runtime.background_tasks.shutdown_and_join().await.unwrap();
    }
}
