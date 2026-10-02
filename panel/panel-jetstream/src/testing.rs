//! Disposable JetStream namespaces for integration tests.
//!
//! Tests use the server named by [`NATS_URL_ENV`] and are skipped without
//! one, unless `PANEL_REQUIRE_INTEGRATION_SERVICES` is set.

use crate::{ensure_streams, JetStreamSettings};
use async_nats::jetstream::{self, Context};
use std::sync::Arc;

pub const NATS_URL_ENV: &str = "PANEL_TEST_NATS_URL";
const REQUIRE_ENV: &str = "PANEL_REQUIRE_INTEGRATION_SERVICES";

/// A JetStream context whose streams and subjects are unique to one test.
pub struct TestBroker {
    pub context: Context,
    pub settings: Arc<JetStreamSettings>,
}

impl TestBroker {
    pub async fn create() -> Option<Self> {
        let Ok(url) = std::env::var(NATS_URL_ENV) else {
            assert!(
                std::env::var_os(REQUIRE_ENV).is_none(),
                "{REQUIRE_ENV} is set but {NATS_URL_ENV} is not"
            );
            eprintln!("skipping: {NATS_URL_ENV} is not set");
            return None;
        };
        let client = async_nats::connect(url)
            .await
            .expect("test broker is reachable");
        let context = jetstream::new(client);
        let suffix = uuid::Uuid::new_v4().simple().to_string()[..12].to_owned();
        let settings = Arc::new(
            JetStreamSettings::default()
                .with_prefixes(
                    format!("t{suffix}"),
                    format!("T{}", suffix.to_ascii_uppercase()),
                )
                .expect("test prefixes are valid"),
        );
        ensure_streams(&context, &settings)
            .await
            .expect("test streams can be provisioned");
        Some(Self { context, settings })
    }

    /// Deletes the test's streams and their consumers.
    pub async fn drop(self) {
        for stream in [
            self.settings.events_stream(),
            self.settings.dead_letter_stream(),
        ] {
            let _ = self.context.delete_stream(stream).await;
        }
    }
}
