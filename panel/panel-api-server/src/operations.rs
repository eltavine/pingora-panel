//! Gateway operations and refused requests recorded in this service's
//! outbox for the audit trail.

use async_trait::async_trait;
use panel_api::{AccessAudit, Refusal};
use panel_application::{CommandContext, OperationLog, RequestScope};
use panel_identity::Principal;
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
            .record_named(event_type, target, &context.scope(), context.actor(), &data)
            .await;
    }
}

#[async_trait]
impl AccessAudit for OutboxOperations {
    async fn denied(&self, principal: &Principal, refusal: &Refusal, scope: &RequestScope) {
        self.0
            .record_named(
                "identity.access.denied",
                ("account", &principal.account.to_string()),
                scope,
                principal.actor(),
                refusal,
            )
            .await;
    }
}
