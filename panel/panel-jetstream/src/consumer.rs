use crate::{
    dead_letter::{DeadLetterQueue, ParkedSource},
    error::broker_error,
    message::decode_event,
    JetStreamSettings,
};
use async_nats::jetstream::{
    consumer::{pull, AckPolicy, DeliverPolicy, PullConsumer},
    AckKind, Context, Message,
};
use futures_util::{FutureExt, StreamExt};
use panel_errors::{PanelError, Result};
use panel_events::{ConsumerName, EventDelivery, EventHandler, HandlerOutcome};
use std::{future::Future, num::NonZeroU32, panic::AssertUnwindSafe, sync::Arc, time::Duration};

const MIN_RETRY_DELAY: Duration = Duration::from_millis(100);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(15 * 60);

/// Durable consumer definition.
#[derive(Clone, Debug)]
pub struct ConsumerSpec {
    name: ConsumerName,
    event_patterns: Vec<String>,
    ack_wait: Duration,
    max_deliver: NonZeroU32,
    max_ack_pending: u32,
}

impl ConsumerSpec {
    /// `event_patterns` select event subjects, for example `config.>`.
    pub fn new(name: ConsumerName, event_patterns: Vec<String>) -> Result<Self> {
        if event_patterns.is_empty() {
            return Err(PanelError::invalid_argument(
                "a consumer needs at least one event pattern",
            ));
        }
        Ok(Self {
            name,
            event_patterns,
            ack_wait: Duration::from_secs(30),
            max_deliver: NonZeroU32::new(5).expect("five is non-zero"),
            max_ack_pending: 64,
        })
    }

    /// Time a delivery may stay unanswered before JetStream redelivers it.
    /// Long handlers extend it automatically with progress acknowledgements.
    pub fn with_ack_wait(mut self, ack_wait: Duration) -> Self {
        self.ack_wait = ack_wait.max(Duration::from_secs(1));
        self
    }

    /// Delivery attempts before an event is parked in the dead-letter queue.
    pub fn with_max_deliver(mut self, max_deliver: NonZeroU32) -> Self {
        self.max_deliver = max_deliver;
        self
    }

    pub fn with_max_ack_pending(mut self, max_ack_pending: u32) -> Self {
        self.max_ack_pending = max_ack_pending.max(1);
        self
    }

    pub fn name(&self) -> &ConsumerName {
        &self.name
    }
}

/// A durable pull consumer that drives an [`EventHandler`].
///
/// Acknowledgements are explicit and confirmed by the server. A retry is
/// redelivered after the delay the handler asked for; on the final attempt
/// the event is parked in the dead-letter queue instead, and an event that
/// cannot be decoded is parked immediately.
pub struct JetStreamConsumer {
    spec: ConsumerSpec,
    consumer: PullConsumer,
    dead_letters: DeadLetterQueue,
}

impl JetStreamConsumer {
    /// Creates or updates the durable consumer on the event stream.
    pub async fn ensure(
        context: &Context,
        settings: Arc<JetStreamSettings>,
        spec: ConsumerSpec,
    ) -> Result<Self> {
        let mut filter_subjects = spec
            .event_patterns
            .iter()
            .map(|pattern| settings.event_filter(pattern))
            .collect::<Result<Vec<_>>>()?;
        filter_subjects.push(settings.replay_subject(&spec.name));
        let config = pull::Config {
            durable_name: Some(spec.name.as_str().to_owned()),
            deliver_policy: DeliverPolicy::All,
            ack_policy: AckPolicy::Explicit,
            ack_wait: spec.ack_wait,
            max_deliver: i64::from(spec.max_deliver.get()),
            max_ack_pending: i64::from(spec.max_ack_pending),
            filter_subjects,
            ..pull::Config::default()
        };
        let stream = context
            .get_stream_no_info(settings.events_stream())
            .await
            .map_err(|error| broker_error("event stream lookup", error))?;
        // Creating an existing durable with the same name updates it in place.
        let consumer = stream
            .create_consumer(config)
            .await
            .map_err(|error| broker_error("consumer provisioning", error))?;
        Ok(Self {
            spec,
            consumer,
            dead_letters: DeadLetterQueue::new(context.clone(), settings),
        })
    }

    pub fn spec(&self) -> &ConsumerSpec {
        &self.spec
    }

    /// Consumes until `shutdown` resolves. A delivery in progress finishes
    /// before the consumer stops.
    pub async fn run(
        &self,
        handler: Arc<dyn EventHandler>,
        shutdown: impl Future<Output = ()> + Send,
    ) -> Result<()> {
        let mut messages = self
            .consumer
            .messages()
            .await
            .map_err(|error| broker_error("consumer subscription", error))?;
        tokio::pin!(shutdown);
        loop {
            let next = tokio::select! {
                () = &mut shutdown => return Ok(()),
                next = messages.next() => next,
            };
            match next {
                Some(Ok(message)) => self.process(handler.as_ref(), message).await,
                Some(Err(error)) => tracing::warn!(
                    event = "consumer_pull_failed",
                    consumer = %self.spec.name,
                    error = %error,
                ),
                None => return Ok(()),
            }
        }
    }

    async fn process(&self, handler: &dyn EventHandler, message: Message) {
        let (sequence, deliveries) = match message.info() {
            Ok(info) => (
                info.stream_sequence,
                u64::try_from(info.delivered).unwrap_or(1),
            ),
            Err(error) => {
                tracing::warn!(event = "consumer_message_unattributed", error = %error);
                return;
            }
        };
        let attempt = u32::try_from(deliveries)
            .ok()
            .and_then(NonZeroU32::new)
            .unwrap_or(NonZeroU32::MIN);
        let outcome = match decode_event(message.headers.as_ref(), &message.payload) {
            Ok(envelope) => {
                let delivery = EventDelivery::new(envelope, self.spec.name.clone(), attempt);
                self.handle_with_progress(handler, &delivery, &message)
                    .await
            }
            Err(error) => HandlerOutcome::dead_letter(format!(
                "message is not a valid CloudEvent: {}",
                error.message
            )),
        };
        let result = match outcome {
            HandlerOutcome::Ack => message.double_ack().await,
            HandlerOutcome::Retry { after, .. } if attempt < self.spec.max_deliver => {
                let after = after.clamp(MIN_RETRY_DELAY, MAX_RETRY_DELAY);
                message.ack_with(AckKind::Nak(Some(after))).await
            }
            HandlerOutcome::Retry { reason, .. } => {
                self.park(
                    &message,
                    sequence,
                    deliveries,
                    &format!("retries exhausted: {reason}"),
                )
                .await
            }
            HandlerOutcome::DeadLetter { reason } => {
                self.park(&message, sequence, deliveries, &reason).await
            }
            _ => message.ack_with(AckKind::Nak(Some(MIN_RETRY_DELAY))).await,
        };
        if let Err(error) = result {
            // Without an acknowledgement JetStream redelivers after the ack
            // wait; idempotent handlers make that safe.
            tracing::warn!(
                event = "consumer_acknowledgement_failed",
                consumer = %self.spec.name,
                stream_sequence = sequence,
                error = %error,
            );
        }
    }

    /// Parks the event, then terminates its delivery. If parking fails the
    /// delivery is retried later instead of being lost.
    async fn park(
        &self,
        message: &Message,
        sequence: u64,
        deliveries: u64,
        reason: &str,
    ) -> std::result::Result<(), async_nats::Error> {
        let parked = self
            .dead_letters
            .park(
                &self.spec.name,
                ParkedSource {
                    subject: message.subject.as_str(),
                    headers: message.headers.as_ref(),
                    payload: &message.payload,
                    sequence,
                    deliveries,
                },
                reason,
            )
            .await;
        match parked {
            Ok(()) => message.double_ack_with(AckKind::Term).await,
            Err(error) => {
                tracing::warn!(
                    event = "dead_letter_failed",
                    consumer = %self.spec.name,
                    stream_sequence = sequence,
                    error_code = %error.code,
                );
                message
                    .ack_with(AckKind::Nak(Some(self.spec.ack_wait)))
                    .await
            }
        }
    }

    /// Runs the handler, extending the ack deadline while it works and
    /// treating a panic as a transient failure.
    async fn handle_with_progress(
        &self,
        handler: &dyn EventHandler,
        delivery: &EventDelivery,
        message: &Message,
    ) -> HandlerOutcome {
        let work = AssertUnwindSafe(handler.handle(delivery)).catch_unwind();
        tokio::pin!(work);
        let mut progress = tokio::time::interval(self.spec.ack_wait / 3);
        progress.tick().await;
        loop {
            tokio::select! {
                result = &mut work => {
                    return result.unwrap_or_else(|_| {
                        tracing::error!(
                            event = "event_handler_panicked",
                            consumer = %self.spec.name,
                            event_id = %delivery.envelope().event_id(),
                        );
                        HandlerOutcome::retry(self.spec.ack_wait, "handler panicked")
                    });
                }
                _ = progress.tick() => {
                    let _ = message.ack_with(AckKind::Progress).await;
                }
            }
        }
    }
}
