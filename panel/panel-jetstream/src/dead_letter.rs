use crate::{
    error::broker_error,
    message::{
        application_headers, decode_event, decode_header_value, encode_header_value, header_value,
    },
    JetStreamSettings,
};
use async_nats::{
    jetstream::{message::PublishMessage, stream::DirectGetErrorKind, Context},
    HeaderMap,
};
use bytes::Bytes;
use chrono::{DateTime, SecondsFormat, Utc};
use futures_util::StreamExt;
use panel_errors::{PanelError, Result};
use panel_events::{ConsumerName, EventEnvelope};
use serde::Deserialize;
use std::{future::Future, sync::Arc};

const HEADER_PREFIX: &str = "panel-dead-letter-";
const REASON: &str = "Panel-Dead-Letter-Reason";
const CONSUMER: &str = "Panel-Dead-Letter-Consumer";
const SOURCE_SUBJECT: &str = "Panel-Dead-Letter-Source-Subject";
const SOURCE_SEQUENCE: &str = "Panel-Dead-Letter-Source-Sequence";
const DELIVERIES: &str = "Panel-Dead-Letter-Deliveries";
const PARKED_AT: &str = "Panel-Dead-Letter-Time";
const REPLAY_OF: &str = "Panel-Replay-Of";
const MAX_REASON_BYTES: usize = 512;

/// The original message of a parked event.
pub(crate) struct ParkedSource<'a> {
    pub subject: &'a str,
    pub headers: Option<&'a HeaderMap>,
    pub payload: &'a Bytes,
    pub sequence: u64,
    pub deliveries: u64,
}

/// An event a consumer could not process, kept for operator review.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct DeadLetterRecord {
    pub sequence: u64,
    pub consumer: Option<ConsumerName>,
    /// `None` when the parked message is not a valid event.
    pub envelope: Option<EventEnvelope>,
    pub reason: String,
    pub deliveries: u64,
    pub source_subject: String,
    pub source_sequence: u64,
    pub parked_at: Option<DateTime<Utc>>,
}

/// Result of replaying a parked event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct ReplayReceipt {
    pub dead_letter_sequence: u64,
    pub replay_sequence: u64,
}

/// The dead-letter queue: parking, inspection, targeted replay and discard.
///
/// JetStream drops a message from a consumer once its delivery limit is
/// reached and has no dead-letter queue of its own, so the product parks
/// such events in a dedicated stream instead of losing them silently.
#[derive(Clone)]
pub struct DeadLetterQueue {
    context: Context,
    settings: Arc<JetStreamSettings>,
}

impl DeadLetterQueue {
    pub fn new(context: Context, settings: Arc<JetStreamSettings>) -> Self {
        Self { context, settings }
    }

    /// Parks a message. The message ID is derived from the consumer and the
    /// source sequence, so parking the same delivery twice stores it once.
    pub(crate) async fn park(
        &self,
        consumer: &ConsumerName,
        source: ParkedSource<'_>,
        reason: &str,
    ) -> Result<()> {
        let mut headers = application_headers(source.headers, HEADER_PREFIX);
        let reason = truncate(reason, MAX_REASON_BYTES);
        headers.insert(REASON, encode_header_value(&reason).as_str());
        headers.insert(CONSUMER, consumer.as_str());
        headers.insert(SOURCE_SUBJECT, source.subject);
        headers.insert(SOURCE_SEQUENCE, source.sequence.to_string().as_str());
        headers.insert(DELIVERIES, source.deliveries.to_string().as_str());
        headers.insert(
            PARKED_AT,
            Utc::now()
                .to_rfc3339_opts(SecondsFormat::Millis, true)
                .as_str(),
        );
        self.context
            .send_publish(
                self.settings.dead_letter_subject(consumer),
                PublishMessage::build()
                    .headers(headers)
                    .payload(source.payload.clone())
                    .message_id(format!("dlq.{consumer}.{}", source.sequence)),
            )
            .await
            .map_err(|error| broker_error("dead-letter publish", error))?
            .await
            .map_err(|error| broker_error("dead-letter acknowledgement", error))?;
        tracing::warn!(
            event = "event_dead_lettered",
            consumer = %consumer,
            source_sequence = source.sequence,
            deliveries = source.deliveries,
            reason = %reason,
            "event parked in the dead-letter queue"
        );
        Ok(())
    }

    /// Up to `limit` parked events of `consumer`, oldest first, after
    /// `after_sequence`.
    pub async fn list(
        &self,
        consumer: &ConsumerName,
        after_sequence: u64,
        limit: usize,
    ) -> Result<Vec<DeadLetterRecord>> {
        let stream = self
            .context
            .get_stream_no_info(self.settings.dead_letter_stream())
            .await
            .map_err(|error| broker_error("dead-letter stream lookup", error))?;
        let subject = self.settings.dead_letter_subject(consumer);
        let mut records = Vec::new();
        let mut next = after_sequence.saturating_add(1);
        while records.len() < limit {
            match stream
                .direct_get_next_for_subject(subject.clone(), Some(next))
                .await
            {
                Ok(message) => {
                    next = message.sequence.saturating_add(1);
                    records.push(record(message.sequence, &message.headers, &message.payload));
                }
                Err(error) if matches!(error.kind(), DirectGetErrorKind::NotFound) => break,
                Err(error) => return Err(broker_error("dead-letter read", error)),
            }
        }
        Ok(records)
    }

    /// Redelivers a parked event to the consumer that parked it, through its
    /// own replay subject so no other consumer sees it again, then removes it
    /// from the queue.
    pub async fn replay(&self, sequence: u64) -> Result<ReplayReceipt> {
        let stream = self
            .context
            .get_stream_no_info(self.settings.dead_letter_stream())
            .await
            .map_err(|error| broker_error("dead-letter stream lookup", error))?;
        let message = stream.direct_get(sequence).await.map_err(|error| {
            if matches!(error.kind(), DirectGetErrorKind::NotFound) {
                PanelError::not_found(format!("dead letter {sequence} does not exist"))
            } else {
                broker_error("dead-letter read", error)
            }
        })?;
        let consumer = header_value(&message.headers, CONSUMER)
            .and_then(|value| ConsumerName::new(value).ok())
            .ok_or_else(|| {
                PanelError::corrupt_state(format!("dead letter {sequence} names no consumer"))
            })?;
        let mut headers = application_headers(Some(&message.headers), HEADER_PREFIX);
        headers.insert(REPLAY_OF, sequence.to_string().as_str());
        let acknowledgement = self
            .context
            .send_publish(
                self.settings.replay_subject(&consumer),
                PublishMessage::build()
                    .headers(headers)
                    .payload(message.payload.clone())
                    .message_id(format!("replay.{sequence}")),
            )
            .await
            .map_err(|error| broker_error("replay publish", error))?
            .await
            .map_err(|error| broker_error("replay acknowledgement", error))?;
        stream
            .delete_message(sequence)
            .await
            .map_err(|error| broker_error("dead-letter removal", error))?;
        tracing::info!(
            event = "dead_letter_replayed",
            consumer = %consumer,
            dead_letter_sequence = sequence,
            replay_sequence = acknowledgement.sequence,
        );
        Ok(ReplayReceipt {
            dead_letter_sequence: sequence,
            replay_sequence: acknowledgement.sequence,
        })
    }

    /// Removes a parked event without redelivering it.
    pub async fn discard(&self, sequence: u64) -> Result<bool> {
        let stream = self
            .context
            .get_stream_no_info(self.settings.dead_letter_stream())
            .await
            .map_err(|error| broker_error("dead-letter stream lookup", error))?;
        match stream.delete_message(sequence).await {
            Ok(deleted) => Ok(deleted),
            Err(error) if error.to_string().to_ascii_lowercase().contains("not found") => Ok(false),
            Err(error) => Err(broker_error("dead-letter removal", error)),
        }
    }

    /// Parks every event that exhausts `consumer`'s delivery limit without a
    /// handler decision, such as a handler that keeps crashing or timing
    /// out, by following JetStream's max-deliveries advisory.
    pub async fn watch_max_deliveries(
        &self,
        consumer: &ConsumerName,
        shutdown: impl Future<Output = ()> + Send,
    ) -> Result<()> {
        let events_stream = self.settings.events_stream();
        let subject =
            format!("$JS.EVENT.ADVISORY.CONSUMER.MAX_DELIVERIES.{events_stream}.{consumer}");
        let mut advisories = self
            .context
            .client()
            .subscribe(subject)
            .await
            .map_err(|error| broker_error("advisory subscription", error))?;
        let stream = self
            .context
            .get_stream_no_info(events_stream)
            .await
            .map_err(|error| broker_error("event stream lookup", error))?;
        tokio::pin!(shutdown);
        loop {
            let advisory = tokio::select! {
                () = &mut shutdown => return Ok(()),
                advisory = advisories.next() => advisory,
            };
            let Some(advisory) = advisory else {
                return Ok(());
            };
            let Ok(advisory) = serde_json::from_slice::<MaxDeliveriesAdvisory>(&advisory.payload)
            else {
                continue;
            };
            match stream.get_raw_message(advisory.stream_seq).await {
                Ok(message) => {
                    self.park(
                        consumer,
                        ParkedSource {
                            subject: message.subject.as_str(),
                            headers: Some(&message.headers),
                            payload: &message.payload,
                            sequence: message.sequence,
                            deliveries: advisory.deliveries,
                        },
                        "maximum deliveries exceeded without a handler decision",
                    )
                    .await?;
                }
                Err(error) => tracing::warn!(
                    event = "dead_letter_source_missing",
                    consumer = %consumer,
                    stream_sequence = advisory.stream_seq,
                    error = %error,
                ),
            }
        }
    }
}

/// JetStream `io.nats.jetstream.advisory.v1.max_deliver`.
#[derive(Deserialize)]
struct MaxDeliveriesAdvisory {
    stream_seq: u64,
    deliveries: u64,
}

fn record(sequence: u64, headers: &HeaderMap, payload: &Bytes) -> DeadLetterRecord {
    let number = |name| {
        header_value(headers, name)
            .and_then(|value| value.parse().ok())
            .unwrap_or_default()
    };
    DeadLetterRecord {
        sequence,
        consumer: header_value(headers, CONSUMER).and_then(|value| ConsumerName::new(value).ok()),
        envelope: decode_event(Some(headers), payload).ok(),
        reason: header_value(headers, REASON)
            .map(decode_header_value)
            .unwrap_or_default(),
        deliveries: number(DELIVERIES),
        source_subject: header_value(headers, SOURCE_SUBJECT)
            .unwrap_or_default()
            .to_owned(),
        source_sequence: number(SOURCE_SEQUENCE),
        parked_at: header_value(headers, PARKED_AT)
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|time| time.with_timezone(&Utc)),
    }
}

fn truncate(value: &str, max_bytes: usize) -> String {
    value
        .char_indices()
        .take_while(|(index, character)| index + character.len_utf8() <= max_bytes)
        .map(|(_, character)| character)
        .collect()
}
