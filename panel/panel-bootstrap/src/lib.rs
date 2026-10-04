#![forbid(unsafe_code)]

//! Idempotent provisioning of an installation: the event streams and
//! service registry in JetStream. Its `pki` mode creates the internal
//! certificate authority and keeps every service's mutual TLS credentials
//! current.

mod pki;

pub use pki::{
    PkiPlan, CERTIFICATE_LIFETIME_MS_ENV, PKI_CHECK_INTERVAL_MS_ENV, PKI_CREDENTIALS_ENV,
    PKI_DIR_ENV,
};

use panel_control_runtime::NATS_URL_ENV;
use panel_errors::Result;
use panel_jetstream::{ensure_streams, JetStreamServiceRegistry, JetStreamSettings};
use panel_service::Environment;

const DEFAULT_NATS_URL: &str = "nats://127.0.0.1:4222";

/// What to provision.
pub struct Plan {
    nats_url: String,
    jetstream: JetStreamSettings,
}

impl Plan {
    pub fn read(env: &mut Environment<'_>) -> Result<Self> {
        Ok(Self {
            nats_url: env
                .string(NATS_URL_ENV)?
                .unwrap_or_else(|| DEFAULT_NATS_URL.into()),
            jetstream: JetStreamSettings::default(),
        })
    }

    pub fn with_jetstream_settings(mut self, settings: JetStreamSettings) -> Self {
        self.jetstream = settings;
        self
    }

    pub async fn apply(&self) -> Result<()> {
        let client = async_nats::connect(self.nats_url.as_str())
            .await
            .map_err(|error| {
                panel_errors::PanelError::unavailable(format!("broker connection failed: {error}"))
            })?;
        let context = async_nats::jetstream::new(client);
        ensure_streams(&context, &self.jetstream).await?;
        JetStreamServiceRegistry::provision(
            &context,
            &self.jetstream,
            JetStreamServiceRegistry::DEFAULT_TTL,
        )
        .await?;
        tracing::info!("event streams and the service registry are provisioned");
        Ok(())
    }
}
