//! Events a service publishes through its outbox. A change writes its event
//! in the transaction that makes it; an attempt that changes nothing in the
//! service, such as a refused change or a call to another process, is
//! recorded on its own so the audit trail sees it too.

use crate::{storage_error, PgOutbox, ServiceDatabase};
use chrono::Utc;
use panel_errors::Result;
use panel_events::{
    Actor, AggregateId, AggregateRef, AggregateType, EventData, EventDraft, EventEnvelope,
    EventOrigin, EventPayload, EventType, EventVersion, Principal, RequestScope, ServiceName,
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
    pub fn event<E: EventData>(
        &self,
        aggregate: (&str, &str),
        scope: &RequestScope,
        actor: &str,
        data: &E,
    ) -> Result<EventEnvelope> {
        self.event_by(aggregate, scope, &Self::user(actor), data)
    }

    /// An event about `aggregate` that `principal` caused within `scope`.
    pub fn event_by<E: EventData>(
        &self,
        aggregate: (&str, &str),
        scope: &RequestScope,
        principal: &Principal,
        data: &E,
    ) -> Result<EventEnvelope> {
        let draft = EventDraft::of(aggregate_ref(aggregate)?, data)?;
        Ok(self.envelope(draft, scope, principal))
    }

    /// Records an event on its own. What it describes already happened, so
    /// a failure is logged rather than returned.
    pub async fn record<E: EventData>(
        &self,
        aggregate: (&str, &str),
        scope: &RequestScope,
        actor: &str,
        data: &E,
    ) {
        self.record_by(aggregate, scope, &Self::user(actor), data)
            .await;
    }

    /// Records an event that `principal` caused on its own; see
    /// [`record`](Self::record).
    pub async fn record_by<E: EventData>(
        &self,
        aggregate: (&str, &str),
        scope: &RequestScope,
        principal: &Principal,
        data: &E,
    ) {
        let event = self.event_by(aggregate, scope, principal, data);
        self.append_alone(E::TYPE, event).await;
    }

    /// An event whose data has no definition yet.
    pub fn event_named<T: Serialize>(
        &self,
        event_type: &str,
        aggregate: (&str, &str),
        scope: &RequestScope,
        actor: &str,
        data: &T,
    ) -> Result<EventEnvelope> {
        self.event_named_by(event_type, aggregate, scope, &Self::user(actor), data)
    }

    /// An event whose data has no definition yet, caused by `principal`.
    pub fn event_named_by<T: Serialize>(
        &self,
        event_type: &str,
        aggregate: (&str, &str),
        scope: &RequestScope,
        principal: &Principal,
        data: &T,
    ) -> Result<EventEnvelope> {
        let draft = EventDraft::new(
            EventType::new(event_type)?,
            EventVersion::V1,
            aggregate_ref(aggregate)?,
            EventPayload::json(data)?,
        );
        Ok(self.envelope(draft, scope, principal))
    }

    /// Records an event whose data has no definition yet on its own.
    pub async fn record_named<T: Serialize>(
        &self,
        event_type: &str,
        aggregate: (&str, &str),
        scope: &RequestScope,
        actor: &str,
        data: &T,
    ) {
        self.record_named_by(event_type, aggregate, scope, &Self::user(actor), data)
            .await;
    }

    /// Records an event whose data has no definition yet on its own, caused
    /// by `principal`.
    pub async fn record_named_by<T: Serialize>(
        &self,
        event_type: &str,
        aggregate: (&str, &str),
        scope: &RequestScope,
        principal: &Principal,
        data: &T,
    ) {
        let event = self.event_named_by(event_type, aggregate, scope, principal, data);
        self.append_alone(event_type, event).await;
    }

    fn envelope(
        &self,
        draft: EventDraft,
        scope: &RequestScope,
        principal: &Principal,
    ) -> EventEnvelope {
        EventEnvelope::new(
            draft,
            EventOrigin::scoped(self.producer.clone(), scope, principal.clone()),
            Utc::now(),
        )
    }

    async fn append_alone(&self, event_type: &str, event: Result<EventEnvelope>) {
        let result = async {
            let event = event?;
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

fn aggregate_ref((kind, id): (&str, &str)) -> Result<AggregateRef> {
    Ok(AggregateRef::new(
        AggregateType::new(kind)?,
        AggregateId::new(id)?,
    ))
}
