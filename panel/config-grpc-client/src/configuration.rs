use crate::{ConfigPublicationClient, MAX_MESSAGE_BYTES};
use async_trait::async_trait;
use config_proto_codec as codec;
use panel_application::{
    ApplyOutcome, ApplyRequest, CommandContext, ConfigurationChange, ConfigurationOutput,
    ConfigurationPort, ConfigurationRead, RequestScope,
};
use panel_contracts::config::v1::{self as wire, configuration_client::ConfigurationClient};
use panel_errors::Result;
use panel_service::status_error;
use tonic::transport::Channel;

fn output(
    content: Vec<u8>,
    etag: String,
    state: Option<wire::Draft>,
) -> Result<ConfigurationOutput> {
    Ok(ConfigurationOutput {
        content,
        etag: (!etag.is_empty()).then_some(etag),
        draft: codec::decode_draft(state)?,
    })
}

impl ConfigPublicationClient {
    fn configuration(&self) -> ConfigurationClient<Channel> {
        ConfigurationClient::new(self.channel.clone())
            .max_decoding_message_size(MAX_MESSAGE_BYTES)
            .max_encoding_message_size(MAX_MESSAGE_BYTES)
    }
}

#[async_trait]
impl ConfigurationPort for ConfigPublicationClient {
    async fn read(
        &self,
        scope: RequestScope,
        read: ConfigurationRead,
    ) -> Result<ConfigurationOutput> {
        let request = wire::ReadRequest {
            context: Some(codec::encode_scope(&scope)),
            operation: read.operation,
            resource: read.resource,
            parameters: read.parameters,
        };
        let response = self
            .configuration()
            .read(self.request(request, scope.trace_context()))
            .await
            .map_err(status_error)?
            .into_inner();
        codec::decode_error(response.error)?;
        output(response.content, response.etag, response.draft)
    }

    async fn change(
        &self,
        context: CommandContext,
        change: ConfigurationChange,
    ) -> Result<ConfigurationOutput> {
        let request = wire::ChangeRequest {
            context: Some(codec::encode_command(&context)),
            operation: change.operation,
            resource: change.resource,
            if_match: change.if_match.unwrap_or_default(),
            content: change.content,
        };
        let response = self
            .configuration()
            .change(self.request(request, context.trace_context()))
            .await
            .map_err(status_error)?
            .into_inner();
        codec::decode_error(response.error)?;
        output(response.content, response.etag, response.draft)
    }

    async fn apply(&self, context: CommandContext, request: ApplyRequest) -> Result<ApplyOutcome> {
        let bypass = request.bypass.unwrap_or_default();
        let wire_request = wire::ApplyRequest {
            context: Some(codec::encode_command(&context)),
            expected_version: request.expected_version,
            note: request.note.unwrap_or_default(),
            dry_run: request.dry_run,
            bypass_reason: bypass.reason,
            bypass_incident: bypass.incident,
        };
        let response = self
            .configuration()
            .apply(self.request(wire_request, context.trace_context()))
            .await
            .map_err(status_error)?
            .into_inner();
        codec::decode_apply_outcome(response)
    }
}
