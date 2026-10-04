//! The configuration API over gRPC: requests decode into the configuration
//! port's terms and what it answers encodes back, so the use cases behind
//! the port know nothing of the transport.
//!
//! Application failures travel in each response's `error` field; transport
//! status codes are left to the transport.

use config_proto_codec as codec;
use panel_config_api::{
    ApplyRequest, ApprovalBypass, ConfigurationChange, ConfigurationOutput, ConfigurationPort,
};
use panel_contracts::config::v1::{self as wire, configuration_server::Configuration};
use panel_errors::Result;
use panel_service::{decode_command, decode_scope, trace_context};
use std::sync::Arc;
use tonic::{Request, Response, Status};

/// Serves a configuration port over gRPC.
pub struct ConfigurationTransport {
    port: Arc<dyn ConfigurationPort>,
}

impl ConfigurationTransport {
    pub fn new(port: Arc<dyn ConfigurationPort>) -> Self {
        Self { port }
    }
}

fn encoded(output: ConfigurationOutput) -> (Vec<u8>, String, Option<wire::Draft>) {
    (
        output.content,
        output.etag.unwrap_or_default(),
        Some(codec::encode_draft(&output.draft)),
    )
}

#[tonic::async_trait]
impl Configuration for ConfigurationTransport {
    async fn read(
        &self,
        request: Request<wire::ReadRequest>,
    ) -> std::result::Result<Response<wire::ReadResponse>, Status> {
        let trace = trace_context(request.metadata());
        let request = request.into_inner();
        let result: Result<_> = async {
            let scope = decode_scope(request.context, trace)?;
            let query = codec::decode_query(&request.query)?;
            self.port.read(scope, query).await
        }
        .await;
        Ok(Response::new(match result.map(encoded) {
            Ok((content, etag, draft)) => wire::ReadResponse {
                content,
                etag,
                draft,
                error: None,
            },
            Err(error) => wire::ReadResponse {
                error: Some((&error).into()),
                ..wire::ReadResponse::default()
            },
        }))
    }

    async fn change(
        &self,
        request: Request<wire::ChangeRequest>,
    ) -> std::result::Result<Response<wire::ChangeResponse>, Status> {
        let trace = trace_context(request.metadata());
        let request = request.into_inner();
        let result: Result<_> = async {
            let context = decode_command(request.context, trace)?;
            let change = ConfigurationChange {
                command: codec::decode_change(&request.command)?,
                if_match: Some(request.if_match).filter(|tag| !tag.is_empty()),
            };
            self.port.change(context, change).await
        }
        .await;
        Ok(Response::new(match result.map(encoded) {
            Ok((content, etag, draft)) => wire::ChangeResponse {
                content,
                etag,
                draft,
                error: None,
            },
            Err(error) => wire::ChangeResponse {
                error: Some((&error).into()),
                ..wire::ChangeResponse::default()
            },
        }))
    }

    async fn apply(
        &self,
        request: Request<wire::ApplyRequest>,
    ) -> std::result::Result<Response<wire::ApplyResponse>, Status> {
        let trace = trace_context(request.metadata());
        let request = request.into_inner();
        let result: Result<_> = async {
            let context = decode_command(request.context, trace)?;
            let mut apply = ApplyRequest::new(request.expected_version);
            if !request.note.is_empty() {
                apply = apply.with_note(request.note);
            }
            if request.dry_run {
                apply = apply.dry_run();
            }
            if !request.bypass_reason.is_empty() || !request.bypass_incident.is_empty() {
                apply = apply.bypassing(ApprovalBypass::new(
                    request.bypass_reason,
                    request.bypass_incident,
                ));
            }
            self.port.apply(context, apply).await
        }
        .await;
        Ok(Response::new(match result {
            Ok(outcome) => codec::encode_apply_outcome(&outcome),
            Err(error) => wire::ApplyResponse {
                error: Some((&error).into()),
                ..wire::ApplyResponse::default()
            },
        }))
    }
}
