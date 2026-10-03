//! Gateway operations recorded in this service's outbox for the audit trail.

use async_trait::async_trait;
use panel_application::{CommandContext, OperationLog};
use panel_postgres::EventLog;
use serde_json::Value;

pub struct OutboxOperations(pub EventLog);

#[async_trait]
impl OperationLog for OutboxOperations {
    async fn record(
        &self,
        context: &CommandContext,
        event_type: &str,
        target: (&str, &str),
        data: Value,
    ) {
        self.0
            .record(event_type, target, &context.scope(), context.actor(), &data)
            .await;
    }
}
