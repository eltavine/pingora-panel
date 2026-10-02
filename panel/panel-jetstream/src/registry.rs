use crate::{error::broker_error, JetStreamSettings};
use async_nats::jetstream::{kv, stream::StorageType, Context};
use async_trait::async_trait;
use chrono::Utc;
use futures_util::TryStreamExt;
use panel_errors::{PanelError, Result};
use panel_platform::{ServiceDescriptor, ServiceDirectory, ServiceListing, ServiceRegistrar};
use panel_platform_codec::{decode_descriptor_bytes, encode_descriptor_bytes};
use std::time::Duration;

const MAX_DESCRIPTOR_BYTES: i32 = 64 * 1024;

/// Live service instances in a JetStream key-value bucket.
///
/// Each instance owns the key `<service>.<instance id>` holding its
/// Protobuf-encoded descriptor. The bucket keeps one revision per key and
/// expires it `ttl` after the last refresh, so an instance that stops without
/// deregistering disappears on its own. Registrations are rebuilt by the
/// next refresh, so the bucket is kept in memory.
pub struct JetStreamServiceRegistry {
    store: kv::Store,
}

impl JetStreamServiceRegistry {
    /// Creates or reconfigures the registry bucket.
    pub async fn provision(
        context: &Context,
        settings: &JetStreamSettings,
        ttl: Duration,
    ) -> Result<Self> {
        if ttl.is_zero() {
            return Err(PanelError::invalid_argument(
                "service registrations need a non-zero expiry",
            ));
        }
        let store = context
            .create_or_update_key_value(kv::Config {
                bucket: settings.service_bucket(),
                description: "Live Pingora Panel service instances".into(),
                history: 1,
                max_age: ttl,
                max_value_size: MAX_DESCRIPTOR_BYTES,
                storage: StorageType::Memory,
                ..kv::Config::default()
            })
            .await
            .map_err(|error| broker_error("service registry provisioning", error))?;
        Ok(Self { store })
    }

    fn key(descriptor: &ServiceDescriptor) -> String {
        format!("{}.{}", descriptor.service(), descriptor.instance_id())
    }
}

#[async_trait]
impl ServiceRegistrar for JetStreamServiceRegistry {
    async fn register(&self, descriptor: &ServiceDescriptor) -> Result<()> {
        self.store
            .put(
                Self::key(descriptor),
                encode_descriptor_bytes(descriptor).into(),
            )
            .await
            .map(drop)
            .map_err(|error| broker_error("service registration", error))
    }

    async fn deregister(&self, descriptor: &ServiceDescriptor) -> Result<()> {
        self.store
            .purge(Self::key(descriptor))
            .await
            .map_err(|error| broker_error("service deregistration", error))
    }
}

#[async_trait]
impl ServiceDirectory for JetStreamServiceRegistry {
    async fn list(&self) -> Result<ServiceListing> {
        let observed_at = Utc::now();
        let keys = self
            .store
            .keys()
            .await
            .map_err(|error| broker_error("service listing", error))?
            .try_collect::<Vec<_>>()
            .await
            .map_err(|error| broker_error("service listing", error))?;
        let mut services = Vec::with_capacity(keys.len());
        for key in keys {
            let Some(value) = self
                .store
                .get(key.as_str())
                .await
                .map_err(|error| broker_error("service listing", error))?
            else {
                continue;
            };
            match decode_descriptor_bytes(value.as_ref()) {
                Ok(descriptor) => services.push(descriptor),
                Err(error) => tracing::warn!(
                    key,
                    error_code = %error.code,
                    "ignoring an undecodable service registration"
                ),
            }
        }
        Ok(ServiceListing::new(observed_at, services))
    }
}
