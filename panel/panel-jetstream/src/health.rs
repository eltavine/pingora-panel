use async_nats::{connection::State, jetstream::Context};
use async_trait::async_trait;
use panel_health::{CheckOutcome, ComponentType, HealthCheck};

/// Passes while the client is connected and the account can use JetStream.
pub struct JetStreamHealthCheck {
    context: Context,
}

impl JetStreamHealthCheck {
    pub fn new(context: Context) -> Self {
        Self { context }
    }
}

#[async_trait]
impl HealthCheck for JetStreamHealthCheck {
    fn component(&self) -> &str {
        "nats"
    }

    fn component_type(&self) -> ComponentType {
        ComponentType::Component
    }

    async fn check(&self) -> CheckOutcome {
        if self.context.client().connection_state() != State::Connected {
            return CheckOutcome::fail("not connected");
        }
        match self.context.query_account().await {
            Ok(_) => CheckOutcome::pass(),
            Err(_) => CheckOutcome::fail("JetStream is unavailable"),
        }
    }
}
