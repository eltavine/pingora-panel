//! Alert rules, their evaluation and the notifications they send
//! (ADR 0027).

mod channels;
mod evaluator;
mod measures;
mod model;
mod notifier;
mod payload;
mod rules;
mod service;

pub use channels::{AlertChannels, ChannelKind, ChannelRecord};
pub use evaluator::Evaluator;
pub use model::{Comparison, Measure, RuleSpec, Severity, State};
pub use notifier::{NotificationRecord, Notifier, TestOutcome};
pub use payload::Notices;
pub use rules::{AlertRules, RuleRecord};
pub use service::AlertsService;

use panel_errors::{PanelError, Result};
use panel_events::{EventData, Principal, RequestScope};
use panel_postgres::{EventLog, PgOutbox};
use sqlx::PgConnection;

/// Who changed something, and within which request.
#[derive(Clone, Copy)]
pub struct Cause<'a> {
    pub scope: &'a RequestScope,
    pub principal: &'a Principal,
}

/// Appends an event about `aggregate` in the transaction of the change.
async fn publish<E: EventData>(
    events: &EventLog,
    connection: &mut PgConnection,
    cause: Cause<'_>,
    aggregate: (&str, &str),
    data: &E,
) -> Result<()> {
    let event = events.event_by(aggregate, cause.scope, cause.principal, data)?;
    PgOutbox::append(connection, &event).await
}

/// The data of a `*.refused` event.
trait Refusal: EventData {
    fn new(id: &str, operation: &str, error: &PanelError) -> Self;
}

macro_rules! refusals {
    ($($message:ty),* $(,)?) => {$(
        impl Refusal for $message {
            fn new(id: &str, operation: &str, error: &PanelError) -> Self {
                Self {
                    id: id.to_owned(),
                    operation: operation.to_owned(),
                    code: error.code.as_str().to_owned(),
                    message: error.message.clone(),
                }
            }
        }
    )*};
}

refusals!(
    panel_event_contracts::observability::v1::AlertRuleRefused,
    panel_event_contracts::observability::v1::AlertChannelRefused,
);

/// Records the refusal of `operation` on `aggregate` if `result` failed.
async fn refused<E: Refusal, T>(
    events: &EventLog,
    cause: Cause<'_>,
    aggregate: (&str, &str),
    operation: &str,
    result: Result<T>,
) -> Result<T> {
    if let Err(error) = &result {
        let data = E::new(aggregate.1, operation, error);
        events
            .record_by(aggregate, cause.scope, cause.principal, &data)
            .await;
    }
    result
}

/// The version a change expects; 0 accepts any.
fn expected(version: u64) -> Option<u64> {
    (version != 0).then_some(version)
}
