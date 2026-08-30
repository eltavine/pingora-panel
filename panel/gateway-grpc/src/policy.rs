mod request;

pub use request::*;

use panel_contracts::gateway::v1::gateway_engine_server::GatewayEngineServer;
use panel_engine::GatewayEngine;
use panel_errors::{PanelError, Result};
use std::{num::NonZeroUsize, time::Duration};

use crate::GatewayGrpcService;

pub const DEFAULT_MAX_DECODING_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_MAX_ENCODING_MESSAGE_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_MAX_CONCURRENT_REQUESTS: usize = 64;
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GatewayTransportPolicy {
    max_decoding_message_bytes: NonZeroUsize,
    max_encoding_message_bytes: NonZeroUsize,
    max_concurrent_requests: NonZeroUsize,
    request_timeout: Duration,
}

impl GatewayTransportPolicy {
    pub fn new(
        max_decoding_message_bytes: usize,
        max_encoding_message_bytes: usize,
        max_concurrent_requests: usize,
        request_timeout: Duration,
    ) -> Result<Self> {
        let positive = |value, name| {
            NonZeroUsize::new(value)
                .ok_or_else(|| PanelError::invalid_argument(format!("{name} must be non-zero")))
        };
        if request_timeout.is_zero() {
            return Err(PanelError::invalid_argument(
                "gRPC request timeout must be non-zero",
            ));
        }
        Ok(Self {
            max_decoding_message_bytes: positive(
                max_decoding_message_bytes,
                "gRPC decoding message limit",
            )?,
            max_encoding_message_bytes: positive(
                max_encoding_message_bytes,
                "gRPC encoding message limit",
            )?,
            max_concurrent_requests: positive(
                max_concurrent_requests,
                "gRPC concurrent request limit",
            )?,
            request_timeout,
        })
    }

    pub fn max_decoding_message_bytes(self) -> usize {
        self.max_decoding_message_bytes.get()
    }

    pub fn max_encoding_message_bytes(self) -> usize {
        self.max_encoding_message_bytes.get()
    }

    pub fn max_concurrent_requests(self) -> usize {
        self.max_concurrent_requests.get()
    }

    pub fn request_timeout(self) -> Duration {
        self.request_timeout
    }

    pub fn gateway_server<E>(
        self,
        service: GatewayGrpcService<E>,
    ) -> GatewayEngineServer<GatewayGrpcService<E>>
    where
        E: GatewayEngine + ?Sized + 'static,
    {
        GatewayEngineServer::new(service)
            .max_decoding_message_size(self.max_decoding_message_bytes())
            .max_encoding_message_size(self.max_encoding_message_bytes())
    }
}

impl Default for GatewayTransportPolicy {
    fn default() -> Self {
        Self {
            max_decoding_message_bytes: NonZeroUsize::new(DEFAULT_MAX_DECODING_MESSAGE_BYTES)
                .expect("default decoding limit is non-zero"),
            max_encoding_message_bytes: NonZeroUsize::new(DEFAULT_MAX_ENCODING_MESSAGE_BYTES)
                .expect("default encoding limit is non-zero"),
            max_concurrent_requests: NonZeroUsize::new(DEFAULT_MAX_CONCURRENT_REQUESTS)
                .expect("default concurrency limit is non-zero"),
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_policy_rejects_zero_limits() {
        assert!(GatewayTransportPolicy::new(0, 1, 1, Duration::from_secs(1)).is_err());
        assert!(GatewayTransportPolicy::new(1, 0, 1, Duration::from_secs(1)).is_err());
        assert!(GatewayTransportPolicy::new(1, 1, 0, Duration::from_secs(1)).is_err());
        assert!(GatewayTransportPolicy::new(1, 1, 1, Duration::ZERO).is_err());
    }
}
