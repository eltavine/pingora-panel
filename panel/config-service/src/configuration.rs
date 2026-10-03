use crate::{
    draft::{ChangeOutput, ChangeRequest, DraftState, PgDrafts},
    operations,
};
use chrono::{DateTime, Utc};
use config_proto_codec as codec;
use panel_application::{
    ActivatedDeployment, CommandContext, ConfigDocument, ContentHash, GatewayUseCases,
};
use panel_config_model::compile;
use panel_contracts::config::v1::{self as wire, configuration_server::Configuration};
use panel_domain::RevisionId;
use panel_engine::{validate_engine_ir, EngineCapability};
use panel_errors::{PanelError, Result, ValidationReport};
use panel_ir::IR_SCHEMA_VERSION;
use panel_service::trace_context;
use std::sync::Arc;
use tonic::{Request, Response, Status};

/// Serves the configuration API: reads and changes of the draft, and applying
/// it to the gateway through the publication use cases.
///
/// Application failures travel in each response's `error` field; transport
/// status codes are left to the transport.
pub struct ConfigurationService {
    drafts: PgDrafts,
    publication: Arc<dyn GatewayUseCases>,
}

impl ConfigurationService {
    pub fn new(drafts: PgDrafts, publication: Arc<dyn GatewayUseCases>) -> Self {
        Self {
            drafts,
            publication,
        }
    }

    async fn apply_draft(
        &self,
        context: CommandContext,
        expected_version: u64,
    ) -> Result<(
        DraftState,
        std::result::Result<ActivatedDeployment, ValidationReport>,
    )> {
        let draft = self.drafts.load().await?;
        if expected_version != 0 && expected_version != draft.version {
            return Err(PanelError::conflict(format!(
                "the draft is at version {}, not {expected_version}",
                draft.version
            )));
        }
        if draft.version == 0 {
            return Err(PanelError::precondition_failed(
                "the draft has no changes to apply",
            ));
        }
        let snapshot = match compile(&draft.model, RevisionId::new(draft.version)) {
            Ok(snapshot) => snapshot,
            Err(diagnostics) => {
                return Ok((draft, Err(ValidationReport::from_diagnostics(diagnostics))))
            }
        };
        let declared = snapshot
            .required_capabilities()
            .iter()
            .map(|capability| {
                EngineCapability::new(capability.name.clone(), capability.version.clone())
            })
            .collect();
        let report = validate_engine_ir(&snapshot, &declared)?;
        if !report.valid {
            return Ok((draft, Err(report)));
        }
        let document = ConfigDocument::new(
            IR_SCHEMA_VERSION,
            "application/json",
            serde_json::to_vec(&snapshot).map_err(|error| {
                PanelError::internal(format!("snapshot cannot be encoded: {error}"))
            })?,
        )?;
        let status = self.publication.status().await?;
        let prepared = self.publication.prepare(context.clone(), document).await?;
        let activated = self
            .publication
            .activate(
                context.clone(),
                prepared.prepare_token().to_owned(),
                status.active_hash().cloned(),
            )
            .await?;
        let draft = self
            .drafts
            .mark_applied(draft.version, &context.scope(), context.actor())
            .await?;
        Ok((draft, Ok(activated)))
    }
}

fn timestamp(value: DateTime<Utc>) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: value.timestamp(),
        nanos: i32::try_from(value.timestamp_subsec_nanos()).unwrap_or(0),
    }
}

fn encode_draft(draft: &DraftState) -> wire::Draft {
    wire::Draft {
        version: draft.version,
        updated_at: Some(timestamp(draft.updated_at)),
        applied_version: draft.applied_version,
        applied_at: draft.applied_at.map(timestamp),
    }
}

/// Binds an idempotency key to the exact change it was first used for.
fn request_hash(request: &wire::ChangeRequest) -> ContentHash {
    let mut bytes = Vec::with_capacity(request.content.len() + 128);
    for part in [
        request.operation.as_bytes(),
        request.resource.as_bytes(),
        request.if_match.as_bytes(),
        &request.content,
    ] {
        bytes.extend_from_slice(&(part.len() as u64).to_be_bytes());
        bytes.extend_from_slice(part);
    }
    ContentHash::from_bytes(&bytes)
}

#[tonic::async_trait]
impl Configuration for ConfigurationService {
    async fn read(
        &self,
        request: Request<wire::ReadRequest>,
    ) -> std::result::Result<Response<wire::ReadResponse>, Status> {
        let trace = trace_context(request.metadata());
        let request = request.into_inner();
        let result: Result<_> = async {
            codec::decode_scope(request.context, trace)?;
            let draft = self.drafts.load().await?;
            let output = operations::read(
                &draft.model,
                &request.operation,
                &request.resource,
                &request.parameters,
            )?;
            Ok((draft, output))
        }
        .await;
        Ok(Response::new(match result {
            Ok((draft, output)) => wire::ReadResponse {
                content: output.content,
                etag: output.etag,
                draft: Some(encode_draft(&draft)),
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
        let hash = request_hash(&request);
        let result: Result<_> = async {
            let context = codec::decode_command(request.context.clone(), trace)?;
            let scope = context.scope();
            self.drafts
                .change(
                    ChangeRequest {
                        idempotency_key: context.idempotency_key(),
                        operation: &request.operation,
                        resource: &request.resource,
                        request_hash: hash,
                        scope: &scope,
                        actor: context.actor(),
                    },
                    |draft| {
                        let (model, output) = operations::change(
                            &draft.model,
                            &request.operation,
                            &request.resource,
                            &request.if_match,
                            &request.content,
                            Utc::now(),
                        )?;
                        Ok((
                            model,
                            ChangeOutput {
                                content: output.content,
                                etag: output.etag,
                            },
                        ))
                    },
                )
                .await
        }
        .await;
        Ok(Response::new(match result {
            Ok((draft, output)) => wire::ChangeResponse {
                content: output.content,
                etag: output.etag,
                draft: Some(encode_draft(&draft)),
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
            let context = codec::decode_command(request.context, trace)?;
            self.apply_draft(context, request.expected_version).await
        }
        .await;
        Ok(Response::new(match result {
            Ok((draft, Ok(deployment))) => wire::ApplyResponse {
                draft: Some(encode_draft(&draft)),
                deployment: Some(codec::encode_activated(&deployment)),
                report: None,
                error: None,
            },
            Ok((draft, Err(report))) => wire::ApplyResponse {
                draft: Some(encode_draft(&draft)),
                deployment: None,
                report: Some(codec::encode_report(&report)),
                error: None,
            },
            Err(error) => wire::ApplyResponse {
                error: Some((&error).into()),
                ..wire::ApplyResponse::default()
            },
        }))
    }
}
