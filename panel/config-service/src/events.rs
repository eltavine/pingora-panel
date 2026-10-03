//! Events this service publishes through its outbox. Changes write their
//! event in the transaction that makes them; attempts that change nothing
//! here, such as a refused change or a call to the gateway, are recorded on
//! their own so the audit trail sees them too.

use chrono::Utc;
use panel_application::RequestScope;
use panel_errors::Result;
use panel_events::{
    Actor, AggregateId, AggregateRef, AggregateType, EventDraft, EventEnvelope, EventOrigin,
    EventPayload, EventType, EventVersion, Principal, ServiceName,
};
use panel_postgres::{storage_error, PgOutbox, ServiceDatabase};
use serde_json::Value;
use sqlx::PgPool;

#[derive(Clone)]
pub struct EventLog {
    pool: PgPool,
    producer: ServiceName,
}

impl EventLog {
    pub fn new(database: &ServiceDatabase, producer: ServiceName) -> Self {
        Self {
            pool: database.pool().clone(),
            producer,
        }
    }

    /// An event about `aggregate`, a type and an ID, caused by `scope`.
    pub fn event(
        &self,
        event_type: &str,
        aggregate: (&str, &str),
        scope: &RequestScope,
        actor: &str,
        data: &Value,
    ) -> Result<EventEnvelope> {
        let principal = Actor::new(actor)
            .map(Principal::user)
            .unwrap_or_else(|_| Principal::unknown());
        Ok(EventEnvelope::new(
            EventDraft::new(
                EventType::new(event_type)?,
                EventVersion::V1,
                AggregateRef::new(
                    AggregateType::new(aggregate.0)?,
                    AggregateId::new(aggregate.1)?,
                ),
                EventPayload::json(data)?,
            ),
            EventOrigin::scoped(self.producer.clone(), scope, principal),
            Utc::now(),
        ))
    }

    /// Records an event on its own. What it describes already happened, so
    /// a failure is logged rather than returned.
    pub async fn record(
        &self,
        event_type: &str,
        aggregate: (&str, &str),
        scope: &RequestScope,
        actor: &str,
        data: Value,
    ) {
        let result = async {
            let event = self.event(event_type, aggregate, scope, actor, &data)?;
            let mut transaction = self.pool.begin().await.map_err(storage_error)?;
            PgOutbox::append(&mut transaction, &event).await?;
            transaction.commit().await.map_err(storage_error)
        }
        .await;
        if let Err(error) = result {
            tracing::warn!(error_code = %error.code, event_type, "event not recorded");
        }
    }
}
