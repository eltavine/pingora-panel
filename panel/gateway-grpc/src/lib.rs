//! Tonic transport adapter for the stable `GatewayEngine` application port.

mod codec;

pub use codec::{decode_snapshot, encode_snapshot};

use panel_contracts::{common::v1 as common, gateway::v1 as wire};
use panel_domain::ContentHash;
use panel_engine::{
    ActivateRequest, EngineCapabilities, GatewayEngine, PrepareRequest, PrepareToken,
};
use panel_errors::{PanelError, Result};
use std::sync::Arc;
use tonic::{Request, Response, Status};

pub struct GatewayGrpcService<E: GatewayEngine + ?Sized> {
    engine: Arc<E>,
}

impl<E: GatewayEngine + ?Sized> Clone for GatewayGrpcService<E> {
    fn clone(&self) -> Self {
        Self {
            engine: Arc::clone(&self.engine),
        }
    }
}

impl<E: GatewayEngine + ?Sized> GatewayGrpcService<E> {
    pub fn new(engine: Arc<E>) -> Self {
        Self { engine }
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
        if let Err(error) = validate_context(request.context.as_ref(), false) {
            return Ok(Response::new(wire::GetCapabilitiesResponse {
                version: None,
                capabilities: Vec::new(),
                error: Some((&error).into()),
            }));
        }
        match self.engine.capabilities().await {
            Ok(capabilities) => Ok(Response::new(wire::GetCapabilitiesResponse {
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
            })),
            Err(error) => Ok(Response::new(wire::GetCapabilitiesResponse {
                version: None,
                capabilities: Vec::new(),
                error: Some((&error).into()),
            })),
        }
    }

    async fn validate(
        &self,
        request: Request<wire::ValidateRequest>,
    ) -> std::result::Result<Response<wire::ValidateResponse>, Status> {
        let request = request.into_inner();
        let snapshot = validate_context(request.context.as_ref(), false).and_then(|_| {
            request
                .snapshot
                .ok_or_else(|| PanelError::invalid_argument("snapshot is required"))
                .and_then(decode_snapshot)
        });
        let response = match snapshot {
            Ok(snapshot) => match self.engine.validate(snapshot).await {
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
        Ok(Response::new(response))
    }

    async fn prepare(
        &self,
        request: Request<wire::PrepareRequest>,
    ) -> std::result::Result<Response<wire::PrepareResponse>, Status> {
        let request = request.into_inner();
        let snapshot = validate_context(request.context.as_ref(), true).and_then(|_| {
            request
                .snapshot
                .ok_or_else(|| PanelError::invalid_argument("snapshot is required"))
                .and_then(decode_snapshot)
        });
        let response = match snapshot {
            Ok(snapshot) => match self.engine.prepare(PrepareRequest { snapshot }).await {
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
        Ok(Response::new(response))
    }

    async fn activate(
        &self,
        request: Request<wire::ActivateRequest>,
    ) -> std::result::Result<Response<wire::ActivateResponse>, Status> {
        let request = request.into_inner();
        let parsed = validate_context(request.context.as_ref(), true).and_then(|_| {
            if request.prepare_token.is_empty() {
                return Err(PanelError::invalid_argument("prepare token is required"));
            }
            let expected = decode_optional_hash(request.expected_active_hash)?;
            Ok(ActivateRequest {
                prepare_token: PrepareToken::new(request.prepare_token),
                expected_active_hash: expected,
            })
        });
        let response = match parsed {
            Ok(request) => match self.engine.activate(request).await {
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
        Ok(Response::new(response))
    }

    async fn abort(
        &self,
        request: Request<wire::AbortRequest>,
    ) -> std::result::Result<Response<wire::AbortResponse>, Status> {
        let request = request.into_inner();
        let parsed = validate_context(request.context.as_ref(), true).and_then(|_| {
            if request.prepare_token.is_empty() {
                Err(PanelError::invalid_argument("prepare token is required"))
            } else {
                Ok(PrepareToken::new(request.prepare_token))
            }
        });
        let response = match parsed {
            Ok(token) => match self.engine.abort(token).await {
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
        Ok(Response::new(response))
    }

    async fn status(
        &self,
        request: Request<wire::StatusRequest>,
    ) -> std::result::Result<Response<wire::StatusResponse>, Status> {
        let request = request.into_inner();
        if let Err(error) = validate_context(request.context.as_ref(), false) {
            return Ok(Response::new(status_error_response(error)));
        }
        let response = match self.engine.status().await {
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
                    active_revision_id: status.active_revision_id.map_or(0, |value| value.get()),
                    active_hash: status.active_hash.as_ref().map(codec::encode_hash),
                    error: None,
                    prepared_count: status.prepared_count as u64,
                }
            }
            Err(error) => status_error_response(error),
        };
        Ok(Response::new(response))
    }
}

fn validate_context(context: Option<&common::RequestContext>, mutation: bool) -> Result<()> {
    let context =
        context.ok_or_else(|| PanelError::invalid_argument("request context is required"))?;
    if context.request_id.is_empty() {
        return Err(PanelError::invalid_argument("request_id is required"));
    }
    if context.actor.is_empty() {
        return Err(PanelError::invalid_argument("actor is required"));
    }
    if context.schema_version != panel_contracts::PROTOCOL_VERSION {
        return Err(PanelError::new(
            panel_errors::ErrorCode::UNSUPPORTED_CAPABILITY,
            format!("request schema {} is not supported", context.schema_version),
        ));
    }
    if mutation && context.idempotency_key.is_empty() {
        return Err(PanelError::invalid_argument(
            "idempotency_key is required for mutations",
        ));
    }
    Ok(())
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_contracts::gateway::v1::gateway_engine_server::GatewayEngine as GatewayEngineService;
    use panel_domain::RevisionId;
    use panel_engine::FakeGatewayEngine;
    use panel_ir::RuntimeSnapshot;

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
        let service = GatewayGrpcService::new(engine);
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
    }
}
