use crate::{ConfigPublicationClient, MAX_MESSAGE_BYTES};
use async_trait::async_trait;
use config_proto_codec as codec;
use panel_application::{CommandContext, RequestScope};
use panel_config_api::{
    ApplyOutcome, ApplyRequest, ConfigurationChange, ConfigurationOutput, ConfigurationPort,
    ConfigurationQuery,
};
use panel_contracts::config::v1::{self as wire, configuration_client::ConfigurationClient};
use panel_errors::Result;
use panel_service::{command_context, request_context, status_error};
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
        query: ConfigurationQuery,
    ) -> Result<ConfigurationOutput> {
        let request = wire::ReadRequest {
            context: Some(request_context(&scope)),
            query: codec::encode_query(&query),
            ..wire::ReadRequest::default()
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
            context: Some(command_context(&context)),
            if_match: change.if_match.unwrap_or_default(),
            command: codec::encode_change(&change.command),
            ..wire::ChangeRequest::default()
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
            context: Some(command_context(&context)),
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
