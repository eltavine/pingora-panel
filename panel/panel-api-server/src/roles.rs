//! The built-in roles, written once the identity schema exists. The check
//! is required, so the API is ready only after it passed.

use async_trait::async_trait;
use identity_postgres::PgIdentityStore;
use panel_health::{CheckOutcome, ComponentType, HealthCheck};
use panel_identity::IdentityStore;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

pub struct BuiltInRoles {
    store: Arc<PgIdentityStore>,
    written: AtomicBool,
    /// Whether a bootstrap token is configured, to explain an empty panel.
    bootstrap: bool,
}

impl BuiltInRoles {
    pub fn new(store: Arc<PgIdentityStore>, bootstrap: bool) -> Self {
        Self {
            store,
            written: AtomicBool::new(false),
            bootstrap,
        }
    }
}

#[async_trait]
impl HealthCheck for BuiltInRoles {
    fn component(&self) -> &str {
        "identity-roles"
    }

    fn component_type(&self) -> ComponentType {
        ComponentType::Component
    }

    async fn check(&self) -> CheckOutcome {
        if self.written.load(Ordering::Acquire) {
            return CheckOutcome::pass();
        }
        if let Err(error) = self.store.sync_roles().await {
            return CheckOutcome::fail(format!("built-in roles not written: {}", error.message));
        }
        self.written.store(true, Ordering::Release);
        if !self.bootstrap && matches!(self.store.has_accounts().await, Ok(false)) {
            tracing::warn!(
                event = "identity_setup_blocked",
                "no account exists and no bootstrap token is configured; set {} to create the first one",
                crate::BOOTSTRAP_TOKEN_ENV
            );
        }
        CheckOutcome::pass()
    }
}
