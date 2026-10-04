use config_proto_codec as codec;
use gateway_proto_codec::decode_hash;
use panel_application::{GatewayUseCases, IdempotencyKey};
use panel_contracts::{
    common::v1 as common,
    config::v1::{self as wire, publication_server::Publication},
};
use panel_errors::{PanelError, Result};
use panel_service::{decode_command, decode_scope, trace_context};
use std::sync::Arc;
use tonic::{Request, Response, Status};

/// Serves the publication API over any `GatewayUseCases`.
///
/// Application failures travel in each response's `error` field with their
/// stable code; transport status codes are left to the transport. An
/// admitted activation keeps running when its caller disconnects, so its
/// receipt is always recorded.
pub struct PublicationService<U: ?Sized> {
    use_cases: Arc<U>,
}

impl<U: ?Sized> PublicationService<U> {
    pub fn new(use_cases: Arc<U>) -> Self {
        Self { use_cases }
    }
}

fn failure(error: &PanelError) -> Option<common::Error> {
    Some(error.into())
}

#[tonic::async_trait]
impl<U> Publication for PublicationService<U>
where
    U: GatewayUseCases + ?Sized + 'static,
{
    async fn validate(
        &self,
        request: Request<wire::ValidateRequest>,
    ) -> std::result::Result<Response<wire::ValidateResponse>, Status> {
        let trace = trace_context(request.metadata());
        let request = request.into_inner();
        let result: Result<_> = async {
            let scope = decode_scope(request.context, trace)?;
            let document = codec::decode_document(request.document)?;
            self.use_cases.validate_with_scope(scope, document).await
        }
        .await;
        Ok(Response::new(match result {
            Ok(report) => wire::ValidateResponse {
                report: Some(codec::encode_report(&report)),
                error: None,
            },
            Err(error) => wire::ValidateResponse {
                report: None,
                error: failure(&error),
            },
        }))
    }

    async fn prepare(
        &self,
        request: Request<wire::PrepareRequest>,
    ) -> std::result::Result<Response<wire::PrepareResponse>, Status> {
        let trace = trace_context(request.metadata());
        let request = request.into_inner();
        let result: Result<_> = async {
            let context = decode_command(request.context, trace)?;
            let document = codec::decode_document(request.document)?;
            self.use_cases.prepare(context, document).await
        }
        .await;
        Ok(Response::new(match result {
            Ok(deployment) => wire::PrepareResponse {
                deployment: Some(codec::encode_prepared(&deployment)),
                error: None,
            },
            Err(error) => wire::PrepareResponse {
                deployment: None,
                error: failure(&error),
            },
        }))
    }

    async fn activate(
        &self,
        request: Request<wire::ActivateRequest>,
    ) -> std::result::Result<Response<wire::ActivateResponse>, Status> {
        let trace = trace_context(request.metadata());
        let request = request.into_inner();
        let use_cases = Arc::clone(&self.use_cases);
        let result: Result<_> = async move {
            let context = decode_command(request.context, trace)?;
            let expected = request
                .expected_active_hash
                .map(|hash| decode_hash(Some(hash)))
                .transpose()?;
            tokio::spawn(async move {
                use_cases
                    .activate(context, request.prepare_token, expected)
                    .await
            })
            .await
            .map_err(|error| {
                PanelError::commit_outcome_unknown(
                    "activation task ended before reporting its outcome",
                )
                .with_source(error)
            })?
        }
        .await;
        Ok(Response::new(match result {
            Ok(deployment) => wire::ActivateResponse {
                deployment: Some(codec::encode_activated(&deployment)),
                error: None,
            },
            Err(error) => wire::ActivateResponse {
                deployment: None,
                error: failure(&error),
            },
        }))
    }

    async fn abort(
        &self,
        request: Request<wire::AbortRequest>,
    ) -> std::result::Result<Response<wire::AbortResponse>, Status> {
        let trace = trace_context(request.metadata());
        let request = request.into_inner();
        let result: Result<_> = async {
            let context = decode_command(request.context, trace)?;
            self.use_cases.abort(context, request.prepare_token).await
        }
        .await;
        Ok(Response::new(match result {
            Ok(outcome) => wire::AbortResponse {
                aborted: codec::encode_abort(outcome),
                error: None,
            },
            Err(error) => wire::AbortResponse {
                aborted: false,
                error: failure(&error),
            },
        }))
    }

    async fn get_gateway_status(
        &self,
        request: Request<wire::GetGatewayStatusRequest>,
    ) -> std::result::Result<Response<wire::GetGatewayStatusResponse>, Status> {
        let trace = trace_context(request.metadata());
        let request = request.into_inner();
        let result: Result<_> = async {
            let scope = decode_scope(request.context, trace)?;
            self.use_cases.status_with_scope(scope).await
        }
        .await;
        Ok(Response::new(match result {
            Ok(status) => wire::GetGatewayStatusResponse {
                status: Some(codec::encode_status(&status)),
                error: None,
            },
            Err(error) => wire::GetGatewayStatusResponse {
                status: None,
                error: failure(&error),
            },
        }))
    }

    async fn get_activation_receipt(
        &self,
        request: Request<wire::GetActivationReceiptRequest>,
    ) -> std::result::Result<Response<wire::GetActivationReceiptResponse>, Status> {
        let trace = trace_context(request.metadata());
        let request = request.into_inner();
        let result: Result<_> = async {
            decode_scope(request.context, trace)?;
            let key = IdempotencyKey::new(request.idempotency_key)?;
            codec::encode_lookup(&self.use_cases.activation_receipt(&key).await?)
        }
        .await;
        Ok(Response::new(match result {
            Ok((state, receipt)) => wire::GetActivationReceiptResponse {
                state: state.into(),
                receipt,
                error: None,
            },
            Err(error) => wire::GetActivationReceiptResponse {
                state: wire::get_activation_receipt_response::State::Unspecified.into(),
                receipt: None,
                error: failure(&error),
            },
        }))
    }
}
