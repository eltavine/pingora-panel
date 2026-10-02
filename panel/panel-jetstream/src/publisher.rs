use crate::{error::broker_error, message::event_headers, JetStreamSettings};
use async_nats::jetstream::{message::PublishMessage, Context};
use async_trait::async_trait;
use panel_errors::Result;
use panel_events::{EventEnvelope, EventPublisher, PublishReceipt};
use std::sync::Arc;

/// Publishes events in binary content mode and waits for the stream's
/// acknowledgement. The event ID is the JetStream message ID, so a repeated
/// publication inside the duplicate window is acknowledged without being
/// stored twice.
#[derive(Clone)]
pub struct JetStreamPublisher {
    context: Context,
    settings: Arc<JetStreamSettings>,
}

impl JetStreamPublisher {
    pub fn new(context: Context, settings: Arc<JetStreamSettings>) -> Self {
        Self { context, settings }
    }
}

#[async_trait]
impl EventPublisher for JetStreamPublisher {
    async fn publish(&self, envelope: &EventEnvelope) -> Result<PublishReceipt> {
        let (headers, data) = event_headers(envelope);
        let subject = self
            .settings
            .event_subject(envelope.event_type(), envelope.event_version());
        let acknowledgement = self
            .context
            .send_publish(
                subject,
                PublishMessage::build()
                    .headers(headers)
                    .payload(data.into())
                    .message_id(envelope.event_id().to_string()),
            )
            .await
            .map_err(|error| broker_error("publish", error))?
            .await
            .map_err(|error| broker_error("publish acknowledgement", error))?;
        Ok(PublishReceipt::new(
            Some(acknowledgement.sequence),
            acknowledgement.duplicate,
        ))
    }
}
