use panel_errors::Result;
use serde::{Deserialize, Serialize};

pub use panel_context::{Actor, IdempotencyKey, RequestDeadline, RequestId};

/// Authenticated command metadata shared by every mutating surface.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommandContext {
    request_id: RequestId,
    correlation_id: RequestId,
    actor: Actor,
    deadline: RequestDeadline,
    idempotency_key: IdempotencyKey,
}

impl CommandContext {
    pub fn new(
        request_id: RequestId,
        correlation_id: RequestId,
        actor: impl Into<String>,
        deadline: RequestDeadline,
        idempotency_key: IdempotencyKey,
    ) -> Result<Self> {
        Ok(Self {
            request_id,
            correlation_id,
            actor: Actor::new(actor)?,
            deadline,
            idempotency_key,
        })
    }

    pub fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    pub fn correlation_id(&self) -> &RequestId {
        &self.correlation_id
    }

    pub fn actor(&self) -> &str {
        self.actor.as_str()
    }

    pub fn deadline(&self) -> &RequestDeadline {
        &self.deadline
    }

    pub fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_context_rejects_unbounded_actor() {
        let context = CommandContext::new(
            RequestId::new("request-1").unwrap(),
            RequestId::new("correlation-1").unwrap(),
            "x".repeat(257),
            RequestDeadline::new("2026-09-06T12:00:00Z").unwrap(),
            IdempotencyKey::new("deploy-1").unwrap(),
        );
        assert!(context.is_err());
    }
}
