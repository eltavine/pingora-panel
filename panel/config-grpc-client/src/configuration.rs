use crate::{ConfigPublicationClient, MAX_MESSAGE_BYTES};
use async_trait::async_trait;
use config_proto_codec as codec;
use panel_application::{
    ApplyOutcome, CommandContext, ConfigurationChange, ConfigurationOutput, ConfigurationPort,
    ConfigurationRead, DraftInfo, RequestScope,
};
use panel_contracts::config::v1::{self as wire, configuration_client::ConfigurationClient};
use panel_errors::{PanelError, Result};
use panel_service::status_error;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tonic::transport::Channel;

fn time(value: Option<prost_types::Timestamp>) -> Option<SystemTime> {
    let value = value?;
    let seconds = u64::try_from(value.seconds).ok()?;
    let nanos = u32::try_from(value.nanos).ok()?;
    UNIX_EPOCH.checked_add(Duration::new(seconds, nanos))
}

fn draft(value: Option<wire::Draft>) -> Result<DraftInfo> {
    let value =
        value.ok_or_else(|| PanelError::internal("the configuration service sent no draft"))?;
    Ok(DraftInfo {
        version: value.version,
        updated_at: time(value.updated_at),
        applied_version: value.applied_version,
        applied_at: time(value.applied_at),
    })
}

fn output(
    content: Vec<u8>,
    etag: String,
    state: Option<wire::Draft>,
) -> Result<ConfigurationOutput> {
    Ok(ConfigurationOutput {
        content,
        etag: (!etag.is_empty()).then_some(etag),
        draft: draft(state)?,
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

    async fn apply(&self, context: CommandContext, expected_version: u64) -> Result<ApplyOutcome> {
        let request = wire::ApplyRequest {
            context: Some(codec::encode_command(&context)),
            expected_version,
        };
        let response = self
            .configuration()
            .apply(self.request(request, context.trace_context()))
            .await
            .map_err(status_error)?
            .into_inner();
        codec::decode_error(response.error)?;
        let draft = draft(response.draft)?;
        Ok(match response.deployment {
            Some(deployment) => ApplyOutcome::Applied {
                draft,
                deployment: codec::decode_activated(Some(deployment))?,
            },
            None => ApplyOutcome::Rejected {
                draft,
                report: codec::decode_report(response.report)?,
            },
        })
    }
}
