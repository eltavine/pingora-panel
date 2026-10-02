#![allow(dead_code)]

use async_trait::async_trait;
use chrono::Utc;
use panel_events::{
    Actor, AggregateId, AggregateRef, AggregateType, EventDelivery, EventDraft, EventEnvelope,
    EventHandler, EventOrigin, EventPayload, EventType, EventVersion, HandlerOutcome, Principal,
    RequestId, ServiceName,
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::Notify;

pub fn event(event_type: &str, sequence: u32) -> EventEnvelope {
    EventEnvelope::new(
        EventDraft::new(
            EventType::new(event_type).unwrap(),
            EventVersion::V1,
            AggregateRef::new(
                AggregateType::new("revision").unwrap(),
                AggregateId::new("42").unwrap(),
            ),
            EventPayload::json(&serde_json::json!({ "sequence": sequence })).unwrap(),
        ),
        EventOrigin::request(
            ServiceName::new("config-service").unwrap(),
            &RequestId::new("req-1").unwrap(),
            &RequestId::new("corr-1").unwrap(),
            Principal::system(Actor::new("config-service").unwrap()),
        ),
        Utc::now(),
    )
}

/// Records deliveries and answers them from a script, then with `fallback`.
pub struct ScriptedHandler {
    script: Mutex<VecDeque<HandlerOutcome>>,
    fallback: HandlerOutcome,
    deliveries: Mutex<Vec<EventDelivery>>,
    delivered: Notify,
}

impl ScriptedHandler {
    pub fn new(script: Vec<HandlerOutcome>, fallback: HandlerOutcome) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into()),
            fallback,
            deliveries: Mutex::new(Vec::new()),
            delivered: Notify::new(),
        })
    }

    pub fn deliveries(&self) -> Vec<EventDelivery> {
        self.deliveries.lock().unwrap().clone()
    }

    pub async fn wait_for(&self, count: usize) {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let notified = self.delivered.notified();
                if self.deliveries.lock().unwrap().len() >= count {
                    return;
                }
                notified.await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "expected {count} deliveries, saw {}",
                self.deliveries().len()
            )
        });
    }
}

#[async_trait]
impl EventHandler for ScriptedHandler {
    async fn handle(&self, delivery: &EventDelivery) -> HandlerOutcome {
        self.deliveries.lock().unwrap().push(delivery.clone());
        let outcome = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| self.fallback.clone());
        self.delivered.notify_waiters();
        outcome
    }
}
