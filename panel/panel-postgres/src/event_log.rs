//! Events a service publishes through its outbox. A change writes its event
//! in the transaction that makes it; an attempt that changes nothing in the
//! service, such as a refused change or a call to another process, is
//! recorded on its own so the audit trail sees it too.

use crate::{storage_error, PgOutbox, ServiceDatabase};
use chrono::Utc;
use panel_errors::Result;
use panel_events::{
    Actor, AggregateId, AggregateRef, AggregateType, EventDraft, EventEnvelope, EventOrigin,
    EventPayload, EventType, EventVersion, Principal, RequestScope, ServiceName,
};
use serde::Serialize;
use sqlx::PgPool;

/// Builds the events of one producer and appends them to its outbox.
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

    /// The user an actor names, or an unknown principal.
    pub fn user(actor: &str) -> Principal {
        Actor::new(actor)
            .map(Principal::user)
            .unwrap_or_else(|_| Principal::unknown())
    }

    /// An event about `aggregate`, a type and an ID, caused by `scope`.
    pub fn event<T: Serialize>(
        &self,
        event_type: &str,
        aggregate: (&str, &str),
        scope: &RequestScope,
        actor: &str,
        data: &T,
    ) -> Result<EventEnvelope> {
        self.event_by(event_type, aggregate, scope, &Self::user(actor), data)
    }

    /// An event about `aggregate` that `principal` caused within `scope`.
    pub fn event_by<T: Serialize>(
        &self,
        event_type: &str,
        aggregate: (&str, &str),
        scope: &RequestScope,
        principal: &Principal,
        data: &T,
    ) -> Result<EventEnvelope> {
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
            EventOrigin::scoped(self.producer.clone(), scope, principal.clone()),
            Utc::now(),
        ))
    }

    /// Records an event on its own. What it describes already happened, so
    /// a failure is logged rather than returned.
    pub async fn record<T: Serialize>(
        &self,
        event_type: &str,
        aggregate: (&str, &str),
        scope: &RequestScope,
        actor: &str,
        data: &T,
    ) {
        self.record_by(event_type, aggregate, scope, &Self::user(actor), data)
            .await;
    }

    /// Records an event that `principal` caused on its own; see
    /// [`record`](Self::record).
    pub async fn record_by<T: Serialize>(
        &self,
        event_type: &str,
        aggregate: (&str, &str),
        scope: &RequestScope,
        principal: &Principal,
        data: &T,
    ) {
        let result = async {
            let event = self.event_by(event_type, aggregate, scope, principal, data)?;
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
