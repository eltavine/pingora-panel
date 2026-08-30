//! Tonic transport adapter for the stable `GatewayEngine` application port.

mod codec;
mod policy;

pub use codec::{decode_snapshot, encode_snapshot};
pub use policy::*;

use panel_contracts::{common::v1 as common, gateway::v1 as wire};
use panel_domain::ContentHash;
use panel_engine::{
    ActivateRequest, EngineCapabilities, GatewayEngine, GatewayEvent, GatewayEventSink,
    GatewayRequestMetadata, GatewayRequestOperation, GatewayRequestOutcome, GatewayRuntimeInfo,
    GatewayRuntimeInfoProvider, NoopGatewayEventSink, PrepareRequest, PrepareToken,
};
use panel_errors::{ErrorCode, PanelError, Result};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    sync::Arc,
    time::Instant,
};
use tonic::{Request, Response, Status};

pub struct GatewayGrpcService<E: GatewayEngine + ?Sized> {
    engine: Arc<E>,
    runtime_info: Option<Arc<dyn GatewayRuntimeInfoProvider>>,
    request_policy: Arc<dyn GatewayRequestPolicy>,
    transport_policy: GatewayTransportPolicy,
    events: Arc<dyn GatewayEventSink>,
    lifetime_dependencies: Vec<Arc<dyn Send + Sync>>,
}

impl<E: GatewayEngine + ?Sized> Clone for GatewayGrpcService<E> {
    fn clone(&self) -> Self {
        Self {
            engine: Arc::clone(&self.engine),
            runtime_info: self.runtime_info.as_ref().map(Arc::clone),
            request_policy: Arc::clone(&self.request_policy),
            transport_policy: self.transport_policy,
            events: Arc::clone(&self.events),
            lifetime_dependencies: self.lifetime_dependencies.clone(),
        }
    }
}

impl<E: GatewayEngine + ?Sized> GatewayGrpcService<E> {
    pub fn new(engine: Arc<E>) -> Self {
        Self {
            engine,
            runtime_info: None,
            request_policy: Arc::new(StandardGatewayRequestPolicy::default()),
            transport_policy: GatewayTransportPolicy::default(),
            events: Arc::new(NoopGatewayEventSink),
            lifetime_dependencies: Vec::new(),
        }
    }

    pub fn with_runtime_info(
        engine: Arc<E>,
        runtime_info: Arc<dyn GatewayRuntimeInfoProvider>,
    ) -> Self {
        Self {
            engine,
            runtime_info: Some(runtime_info),
            request_policy: Arc::new(StandardGatewayRequestPolicy::default()),
            transport_policy: GatewayTransportPolicy::default(),
            events: Arc::new(NoopGatewayEventSink),
            lifetime_dependencies: Vec::new(),
        }
    }

    pub fn with_dependencies(
        engine: Arc<E>,
        runtime_info: Option<Arc<dyn GatewayRuntimeInfoProvider>>,
        request_policy: Arc<dyn GatewayRequestPolicy>,
        transport_policy: GatewayTransportPolicy,
    ) -> Self {
        Self {
            engine,
            runtime_info,
            request_policy,
            transport_policy,
            events: Arc::new(NoopGatewayEventSink),
            lifetime_dependencies: Vec::new(),
        }
    }

    pub fn with_event_sink(mut self, events: Arc<dyn GatewayEventSink>) -> Self {
        self.events = events;
        self
    }

    /// Retain a composition-owned dependency for exactly as long as this service
    /// remains reachable without exposing it through the transport contract.
    pub fn with_lifetime_dependency(mut self, dependency: Arc<dyn Send + Sync>) -> Self {
        self.lifetime_dependencies.push(dependency);
        self
    }

    pub fn transport_policy(&self) -> GatewayTransportPolicy {
        self.transport_policy
    }
}

#[tonic::async_trait]
impl<E> wire::gateway_engine_server::GatewayEngine for GatewayGrpcService<E>
where
    E: GatewayEngine + ?Sized + 'static,
{
    async fn get_capabilities(
        &self,
        request: Request<wire::GetCapabilitiesRequest>,
    ) -> std::result::Result<Response<wire::GetCapabilitiesResponse>, Status> {
        let request = request.into_inner();
        let scope = RequestEventScope::start(
            Arc::clone(&self.events),
            GatewayRequestOperation::GetCapabilities,
            request.context.as_ref(),
        );
        let response = match self
            .request_policy
            .validate(request.context.as_ref(), RequestClass::ReadOnly)
        {
            Ok(budget) => match budget.execute(self.engine.capabilities()).await {
                Ok(capabilities) => wire::GetCapabilitiesResponse {
                    version: Some(version(&capabilities)),
                    capabilities: capabilities
                        .capabilities
                        .iter()
                        .map(|capability| common::Capability {
                            name: capability.name.clone(),
                            version: capability.version.clone(),
                        })
                        .collect(),
                    error: None,
                },
                Err(error) => wire::GetCapabilitiesResponse {
                    version: None,
                    capabilities: Vec::new(),
                    error: Some((&error).into()),
                },
            },
            Err(error) => wire::GetCapabilitiesResponse {
                version: None,
                capabilities: Vec::new(),
                error: Some((&error).into()),
            },
        };
        scope.complete(response.error.as_ref());
        Ok(Response::new(response))
    }

    async fn validate(
        &self,
        request: Request<wire::ValidateRequest>,
    ) -> std::result::Result<Response<wire::ValidateResponse>, Status> {
        let request = request.into_inner();
        let scope = RequestEventScope::start(
            Arc::clone(&self.events),
            GatewayRequestOperation::Validate,
            request.context.as_ref(),
        );
        let snapshot = self
            .request_policy
            .validate(request.context.as_ref(), RequestClass::ReadOnly)
            .and_then(|budget| {
                request
                    .snapshot
                    .ok_or_else(|| PanelError::invalid_argument("snapshot is required"))
                    .and_then(decode_snapshot)
                    .map(|snapshot| (budget, snapshot))
            });
        let response = match snapshot {
            Ok((budget, snapshot)) => match budget.execute(self.engine.validate(snapshot)).await {
                Ok(report) => wire::ValidateResponse {
                    version: self
                        .engine
                        .capabilities()
                        .await
                        .ok()
                        .map(|value| version(&value)),
                    valid: report.valid,
                    diagnostics: report.diagnostics.iter().map(Into::into).collect(),
                    error: None,
                },
                Err(error) => validation_error_response(error),
            },
            Err(error) => validation_error_response(error),
        };
        scope.complete(response.error.as_ref());
        Ok(Response::new(response))
    }

    async fn prepare(
        &self,
        request: Request<wire::PrepareRequest>,
    ) -> std::result::Result<Response<wire::PrepareResponse>, Status> {
        let request = request.into_inner();
        let scope = RequestEventScope::start(
            Arc::clone(&self.events),
            GatewayRequestOperation::Prepare,
            request.context.as_ref(),
        );
        let snapshot = self
            .request_policy
            .validate(request.context.as_ref(), RequestClass::Mutation)
            .and_then(|budget| {
                request
                    .snapshot
                    .ok_or_else(|| PanelError::invalid_argument("snapshot is required"))
                    .and_then(decode_snapshot)
                    .map(|snapshot| (budget, snapshot))
            });
        let response = match snapshot {
            Ok((budget, snapshot)) => match budget
                .execute(self.engine.prepare(PrepareRequest { snapshot }))
                .await
            {
                Ok(receipt) => wire::PrepareResponse {
                    version: self
                        .engine
                        .capabilities()
                        .await
                        .ok()
                        .map(|value| version(&value)),
                    prepare_token: receipt.prepare_token.as_str().into(),
                    revision_id: receipt.revision_id.get(),
                    content_hash: Some(codec::encode_hash(&receipt.content_hash)),
                    diagnostics: Vec::new(),
                    error: None,
                },
                Err(error) => prepare_error_response(error),
            },
            Err(error) => prepare_error_response(error),
        };
        scope.complete(response.error.as_ref());
        Ok(Response::new(response))
    }

    async fn activate(
        &self,
        request: Request<wire::ActivateRequest>,
    ) -> std::result::Result<Response<wire::ActivateResponse>, Status> {
        let request = request.into_inner();
        let scope = RequestEventScope::start(
            Arc::clone(&self.events),
            GatewayRequestOperation::Activate,
            request.context.as_ref(),
        );
        let parsed = self
            .request_policy
            .validate(request.context.as_ref(), RequestClass::Mutation)
            .and_then(|budget| {
                if request.prepare_token.is_empty() {
                    return Err(PanelError::invalid_argument("prepare token is required"));
                }
                let expected = decode_optional_hash(request.expected_active_hash)?;
                Ok((
                    budget,
                    ActivateRequest {
                        prepare_token: PrepareToken::new(request.prepare_token),
                        expected_active_hash: expected,
                    },
                ))
            });
        let response = match parsed {
            Ok((budget, request)) => match budget.execute(self.engine.activate(request)).await {
                Ok(receipt) => wire::ActivateResponse {
                    version: self
                        .engine
                        .capabilities()
                        .await
                        .ok()
                        .map(|value| version(&value)),
                    revision_id: receipt.revision_id.get(),
                    active_hash: Some(codec::encode_hash(&receipt.content_hash)),
                    previous_active_hash: receipt
                        .previous_active_hash
                        .as_ref()
                        .map(codec::encode_hash),
                    error: None,
                },
                Err(error) => activate_error_response(error),
            },
            Err(error) => activate_error_response(error),
        };
        scope.complete(response.error.as_ref());
        Ok(Response::new(response))
    }

    async fn abort(
        &self,
        request: Request<wire::AbortRequest>,
    ) -> std::result::Result<Response<wire::AbortResponse>, Status> {
        let request = request.into_inner();
        let scope = RequestEventScope::start(
            Arc::clone(&self.events),
            GatewayRequestOperation::Abort,
            request.context.as_ref(),
        );
        let parsed = self
            .request_policy
            .validate(request.context.as_ref(), RequestClass::Mutation)
            .and_then(|budget| {
                if request.prepare_token.is_empty() {
                    Err(PanelError::invalid_argument("prepare token is required"))
                } else {
                    Ok((budget, PrepareToken::new(request.prepare_token)))
                }
            });
        let response = match parsed {
            Ok((budget, token)) => match budget.execute(self.engine.abort(token)).await {
                Ok(_) => wire::AbortResponse {
                    version: self
                        .engine
                        .capabilities()
                        .await
                        .ok()
                        .map(|value| version(&value)),
                    aborted: true,
                    error: None,
                },
                Err(error) => wire::AbortResponse {
                    version: None,
                    aborted: false,
                    error: Some((&error).into()),
                },
            },
            Err(error) => wire::AbortResponse {
                version: None,
                aborted: false,
                error: Some((&error).into()),
            },
        };
        scope.complete(response.error.as_ref());
        Ok(Response::new(response))
    }

    async fn status(
        &self,
        request: Request<wire::StatusRequest>,
    ) -> std::result::Result<Response<wire::StatusResponse>, Status> {
        let request = request.into_inner();
        let scope = RequestEventScope::start(
            Arc::clone(&self.events),
            GatewayRequestOperation::Status,
            request.context.as_ref(),
        );
        let response = match self
            .request_policy
            .validate(request.context.as_ref(), RequestClass::ReadOnly)
        {
            Ok(budget) => match budget.execute(self.engine.status()).await {
                Ok(status) => {
                    let capabilities = self.engine.capabilities().await.ok();
                    let state = if status.ready {
                        common::health_status::State::Ready
                    } else {
                        common::health_status::State::NotReady
                    };
                    wire::StatusResponse {
                        version: capabilities.as_ref().map(version),
                        health: Some(common::HealthStatus {
                            state: state.into(),
                            version: capabilities.as_ref().map(version),
                            message: status.message.unwrap_or_else(|| {
                                if status.ready {
                                    "ready".into()
                                } else {
                                    "not ready".into()
                                }
                            }),
                        }),
                        active_revision_id: status
                            .active_revision_id
                            .map_or(0, |value| value.get()),
                        active_hash: status.active_hash.as_ref().map(codec::encode_hash),
                        error: None,
                        prepared_count: status.prepared_count as u64,
                        runtime: self.runtime_info.as_ref().and_then(|provider| {
                            capabilities.as_ref().map(|capabilities| {
                                encode_runtime_info(provider.snapshot(), capabilities)
                            })
                        }),
                    }
                }
                Err(error) => status_error_response(error),
            },
            Err(error) => status_error_response(error),
        };
        scope.complete(response.error.as_ref());
        Ok(Response::new(response))
    }
}

struct RequestEventScope {
    events: Arc<dyn GatewayEventSink>,
    operation: GatewayRequestOperation,
    metadata: GatewayRequestMetadata,
    started_at: Instant,
}

impl RequestEventScope {
    fn start(
        events: Arc<dyn GatewayEventSink>,
        operation: GatewayRequestOperation,
        context: Option<&common::RequestContext>,
    ) -> Self {
        let metadata = context.map_or_else(
            || GatewayRequestMetadata::new("", "", ""),
            |context| {
                GatewayRequestMetadata::new(
                    context.request_id.clone(),
                    context.correlation_id.clone(),
                    context.actor.clone(),
                )
            },
        );
        emit_safely(
            events.as_ref(),
            &GatewayEvent::RequestStarted {
                operation,
                metadata: metadata.clone(),
            },
        );
        Self {
            events,
            operation,
            metadata,
            started_at: Instant::now(),
        }
    }

    fn complete(self, error: Option<&common::Error>) {
        let outcome = error.map_or(GatewayRequestOutcome::Succeeded, |error| {
            GatewayRequestOutcome::Rejected {
                error_code: ErrorCode::new(error.code.clone()),
            }
        });
        emit_safely(
            self.events.as_ref(),
            &GatewayEvent::RequestCompleted {
                operation: self.operation,
                metadata: self.metadata,
                outcome,
                elapsed_micros: u64::try_from(self.started_at.elapsed().as_micros())
                    .unwrap_or(u64::MAX),
            },
        );
    }
}

fn emit_safely(events: &dyn GatewayEventSink, event: &GatewayEvent) {
    let _ = catch_unwind(AssertUnwindSafe(|| events.emit(event)));
}

fn decode_optional_hash(value: Option<common::ContentHash>) -> Result<Option<ContentHash>> {
    value.map(|hash| codec::decode_hash(Some(hash))).transpose()
}

fn version(capabilities: &EngineCapabilities) -> common::Version {
    common::Version {
        product: "pingora-panel".into(),
        component: "gatewayd".into(),
        build: capabilities.build_version.clone(),
        schema: capabilities.schema_version.clone(),
        protocol: capabilities.protocol_version.clone(),
        capability_set: capabilities
            .capabilities
            .iter()
            .map(|value| format!("{}@{}", value.name, value.version))
            .collect::<Vec<_>>()
            .join(","),
    }
}

fn encode_runtime_info(
    runtime: GatewayRuntimeInfo,
    capabilities: &EngineCapabilities,
) -> wire::GatewayRuntimeInfo {
    wire::GatewayRuntimeInfo {
        gateway_version: runtime.gateway_version,
        data_plane_version: capabilities.build_version.clone(),
        adapter_version: capabilities.adapter_version.clone(),
        started_at_unix_seconds: runtime.started_at_unix_seconds,
        uptime_seconds: runtime.uptime_seconds,
        worker_count: runtime.worker_count,
    }
}

fn validation_error_response(error: PanelError) -> wire::ValidateResponse {
    wire::ValidateResponse {
        version: None,
        valid: false,
        diagnostics: error.diagnostics.iter().map(Into::into).collect(),
        error: Some((&error).into()),
    }
}

fn prepare_error_response(error: PanelError) -> wire::PrepareResponse {
    wire::PrepareResponse {
        version: None,
        prepare_token: String::new(),
        revision_id: 0,
        content_hash: None,
        diagnostics: error.diagnostics.iter().map(Into::into).collect(),
        error: Some((&error).into()),
    }
}

fn activate_error_response(error: PanelError) -> wire::ActivateResponse {
    wire::ActivateResponse {
        version: None,
        revision_id: 0,
        active_hash: None,
        previous_active_hash: None,
        error: Some((&error).into()),
    }
}

fn status_error_response(error: PanelError) -> wire::StatusResponse {
    wire::StatusResponse {
        version: None,
        health: Some(common::HealthStatus {
            state: common::health_status::State::NotReady.into(),
            version: None,
            message: error.message.clone(),
        }),
        active_revision_id: 0,
        active_hash: None,
        error: Some((&error).into()),
        prepared_count: 0,
        runtime: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_contracts::gateway::v1::gateway_engine_server::GatewayEngine as GatewayEngineService;
    use panel_domain::RevisionId;
    use panel_engine::{FakeGatewayEngine, GatewayRuntimeInfo};
    use panel_ir::RuntimeSnapshot;
    use std::sync::Mutex;

    #[derive(Default)]
    struct RecordingEventSink(Mutex<Vec<GatewayEvent>>);

    impl GatewayEventSink for RecordingEventSink {
        fn emit(&self, event: &GatewayEvent) {
            self.0.lock().unwrap().push(event.clone());
        }
    }

    fn context(mutation: bool) -> common::RequestContext {
        common::RequestContext {
            request_id: "request-1".into(),
            correlation_id: "correlation-1".into(),
            actor: "operator@example.com".into(),
            deadline: String::new(),
            idempotency_key: if mutation {
                "idempotency-1".into()
            } else {
                String::new()
            },
            schema_version: panel_contracts::PROTOCOL_VERSION.into(),
        }
    }

    struct FixedRuntimeInfo;

    impl GatewayRuntimeInfoProvider for FixedRuntimeInfo {
        fn snapshot(&self) -> GatewayRuntimeInfo {
            GatewayRuntimeInfo {
                gateway_version: "1.2.3".into(),
                started_at_unix_seconds: 1_787_800_000,
                uptime_seconds: 42,
                worker_count: 4,
            }
        }
    }

    #[tokio::test]
    async fn prepare_and_activate_use_only_wire_contracts() {
        let engine = Arc::new(FakeGatewayEngine::with_default_capabilities());
        let service = GatewayGrpcService::new(engine);
        let snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        let prepared = service
            .prepare(Request::new(wire::PrepareRequest {
                context: Some(context(true)),
                snapshot: Some(encode_snapshot(&snapshot)),
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(prepared.error.is_none());

        let activated = service
            .activate(Request::new(wire::ActivateRequest {
                context: Some(context(true)),
                prepare_token: prepared.prepare_token,
                expected_active_hash: None,
            }))
            .await
            .unwrap()
            .into_inner();
        assert!(activated.error.is_none());
        assert_eq!(activated.revision_id, 1);
    }

    #[tokio::test]
    async fn mutations_require_an_idempotency_key() {
        let engine = Arc::new(FakeGatewayEngine::with_default_capabilities());
        let events = Arc::new(RecordingEventSink::default());
        let service = GatewayGrpcService::new(engine)
            .with_event_sink(Arc::clone(&events) as Arc<dyn GatewayEventSink>);
        let mut invalid_context = context(true);
        invalid_context.idempotency_key.clear();
        let response = service
            .prepare(Request::new(wire::PrepareRequest {
                context: Some(invalid_context),
                snapshot: Some(encode_snapshot(&RuntimeSnapshot::empty(RevisionId::new(1)))),
            }))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(
            response.error.unwrap().code,
            panel_errors::ErrorCode::INVALID_ARGUMENT
        );
        let events = events.0.lock().unwrap();
        assert!(matches!(
            &events[0],
            GatewayEvent::RequestStarted {
                operation: GatewayRequestOperation::Prepare,
                metadata
            } if metadata.request_id == "request-1"
                && metadata.correlation_id == "correlation-1"
        ));
        assert!(matches!(
            &events[1],
            GatewayEvent::RequestCompleted {
                operation: GatewayRequestOperation::Prepare,
                outcome: GatewayRequestOutcome::Rejected { error_code },
                ..
            } if error_code.as_str() == panel_errors::ErrorCode::INVALID_ARGUMENT
        ));
    }

    #[tokio::test]
    async fn expired_request_context_deadline_is_rejected_before_dispatch() {
        let engine = Arc::new(FakeGatewayEngine::with_default_capabilities());
        let service = GatewayGrpcService::new(engine);
        let mut expired_context = context(false);
        expired_context.deadline = "1970-01-01T00:00:01Z".into();

        let response = service
            .status(Request::new(wire::StatusRequest {
                context: Some(expired_context),
            }))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(
            response.error.unwrap().code,
            panel_errors::ErrorCode::DEADLINE_EXCEEDED
        );
    }

    #[tokio::test]
    async fn status_combines_engine_and_process_information() {
        let engine = Arc::new(FakeGatewayEngine::with_default_capabilities());
        let service = GatewayGrpcService::with_runtime_info(engine, Arc::new(FixedRuntimeInfo));

        let response = service
            .status(Request::new(wire::StatusRequest {
                context: Some(context(false)),
            }))
            .await
            .unwrap()
            .into_inner();
        let runtime = response.runtime.unwrap();

        assert_eq!(runtime.gateway_version, "1.2.3");
        assert_eq!(runtime.data_plane_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(runtime.adapter_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(runtime.started_at_unix_seconds, 1_787_800_000);
        assert_eq!(runtime.uptime_seconds, 42);
        assert_eq!(runtime.worker_count, 4);
    }
}
