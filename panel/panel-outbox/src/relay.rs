use crate::{OutboxPosition, OutboxSource, OutboxWakeup};
use backon::{BackoffBuilder, ExponentialBackoff, ExponentialBuilder};
use panel_errors::{PanelError, Result};
use panel_events::EventPublisher;
use std::{future::Future, sync::Arc, time::Duration};

/// Relay tuning.
#[derive(Clone, Debug)]
pub struct RelayOptions {
    batch_size: usize,
    idle_poll: Duration,
    min_backoff: Duration,
    max_backoff: Duration,
}

impl RelayOptions {
    pub fn with_batch_size(mut self, batch_size: usize) -> Result<Self> {
        if !(1..=1000).contains(&batch_size) {
            return Err(PanelError::invalid_argument(
                "relay batch size must be within 1..=1000",
            ));
        }
        self.batch_size = batch_size;
        Ok(self)
    }

    /// Longest sleep while idle; commit signals end it early.
    pub fn with_idle_poll(mut self, idle_poll: Duration) -> Self {
        self.idle_poll = idle_poll;
        self
    }

    /// Exponential retry delays with jitter between `min` and `max`.
    pub fn with_backoff(mut self, min: Duration, max: Duration) -> Result<Self> {
        if min.is_zero() || max < min {
            return Err(PanelError::invalid_argument(
                "relay backoff needs 0 < min <= max",
            ));
        }
        self.min_backoff = min;
        self.max_backoff = max;
        Ok(self)
    }

    fn backoff(&self) -> ExponentialBackoff {
        ExponentialBuilder::default()
            .with_min_delay(self.min_backoff)
            .with_max_delay(self.max_backoff)
            .with_jitter()
            .without_max_times()
            .build()
    }
}

impl Default for RelayOptions {
    fn default() -> Self {
        Self {
            batch_size: 100,
            idle_poll: Duration::from_secs(5),
            min_backoff: Duration::from_millis(200),
            max_backoff: Duration::from_secs(30),
        }
    }
}

/// Outcome of one relay pass.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct RelayReport {
    pub published: usize,
    pub failed: Option<OutboxPosition>,
}

/// Publishes committed outbox events in append order.
pub struct OutboxRelay {
    source: Arc<dyn OutboxSource>,
    publisher: Arc<dyn EventPublisher>,
    wakeup: Arc<dyn OutboxWakeup>,
    options: RelayOptions,
}

impl OutboxRelay {
    pub fn new(
        source: Arc<dyn OutboxSource>,
        publisher: Arc<dyn EventPublisher>,
        wakeup: Arc<dyn OutboxWakeup>,
        options: RelayOptions,
    ) -> Self {
        Self {
            source,
            publisher,
            wakeup,
            options,
        }
    }

    /// Publishes one batch in order, stopping at the first failure so no
    /// later event overtakes it.
    pub async fn relay_once(&self) -> Result<RelayReport> {
        let batch = self.source.pending(self.options.batch_size).await?;
        let mut published = Vec::with_capacity(batch.len());
        let mut failed = None;
        for record in &batch {
            match self.publisher.publish(record.envelope()).await {
                Ok(_) => published.push(record.position()),
                Err(error) => {
                    tracing::warn!(
                        event = "outbox_publish_failed",
                        position = record.position().get(),
                        event_id = %record.envelope().event_id(),
                        attempts = record.attempts() + 1,
                        error_code = %error.code,
                        "outbox event could not be published"
                    );
                    self.source
                        .record_failure(record.position(), &error.to_string())
                        .await?;
                    failed = Some(record.position());
                    break;
                }
            }
        }
        if !published.is_empty() {
            self.source.mark_published(&published).await?;
        }
        Ok(RelayReport {
            published: published.len(),
            failed,
        })
    }

    /// Relays until `shutdown` resolves. Errors are logged and retried with
    /// backoff; the relay never exits on a transient failure. A pass under
    /// way when `shutdown` resolves is abandoned, as a crash would leave it:
    /// what it had not marked published is published again.
    pub async fn run(self, shutdown: impl Future<Output = ()> + Send) {
        tokio::pin!(shutdown);
        let mut backoff = self.options.backoff();
        loop {
            let pass = tokio::select! {
                () = &mut shutdown => return,
                pass = self.relay_once() => pass,
            };
            let delay = match pass {
                Ok(report) if report.failed.is_some() => backoff.next(),
                Ok(report) if report.published > 0 => {
                    backoff = self.options.backoff();
                    continue;
                }
                Ok(_) => {
                    backoff = self.options.backoff();
                    tokio::select! {
                        () = &mut shutdown => return,
                        () = self.wakeup.wait(self.options.idle_poll) => continue,
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        event = "outbox_relay_failed",
                        error_code = %error.code,
                        "outbox relay pass failed"
                    );
                    backoff.next()
                }
            }
            .unwrap_or(self.options.max_backoff);
            tokio::select! {
                () = &mut shutdown => return,
                () = tokio::time::sleep(delay) => {}
            }
        }
    }
}
