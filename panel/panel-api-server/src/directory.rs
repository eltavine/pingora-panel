use async_nats::jetstream::Context;
use async_trait::async_trait;
use panel_errors::Result;
use panel_jetstream::{JetStreamServiceRegistry, JetStreamSettings};
use panel_platform::{ServiceDirectory, ServiceListing};
use std::sync::Arc;
use tokio::sync::OnceCell;

/// The broker's service registry, opened on first use so the API starts
/// while the broker is unavailable.
pub(crate) struct RegistryDirectory {
    context: Context,
    settings: Arc<JetStreamSettings>,
    registry: OnceCell<JetStreamServiceRegistry>,
}

impl RegistryDirectory {
    pub(crate) fn new(context: Context, settings: Arc<JetStreamSettings>) -> Self {
        Self {
            context,
            settings,
            registry: OnceCell::new(),
        }
    }
}

#[async_trait]
impl ServiceDirectory for RegistryDirectory {
    async fn list(&self) -> Result<ServiceListing> {
        let registry = self
            .registry
            .get_or_try_init(|| {
                JetStreamServiceRegistry::provision(
                    &self.context,
                    &self.settings,
                    JetStreamServiceRegistry::DEFAULT_TTL,
                )
            })
            .await?;
        registry.list().await
    }
}
