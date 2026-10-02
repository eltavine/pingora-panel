use panel_errors::PanelError;
use std::time::Duration;

// The compiler permits a 2 MiB normalized snapshot. HTTP carries that
// snapshot inside a JSON envelope, so its raw body budget is independently
// larger. Callers can lower either limit explicitly for their deployment.
const DEFAULT_MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
const DEFAULT_UNAVAILABLE_RETRY_AFTER: Duration = Duration::from_secs(5);

/// Resource policy for the public HTTP adapter.
///
/// Private fields plus validated construction allow future limits to be added
/// without exposing a public struct layout to callers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApiConfig {
    max_body_bytes: usize,
    unavailable_retry_after: Duration,
}

impl ApiConfig {
    pub fn new(max_body_bytes: usize) -> Result<Self, PanelError> {
        if max_body_bytes == 0 {
            return Err(PanelError::invalid_argument(
                "API request body limit must be non-zero",
            ));
        }
        Ok(Self {
            max_body_bytes,
            ..Self::default()
        })
    }

    /// Delay advertised to clients whose request was refused because the
    /// service runs degraded; normally the health re-evaluation interval.
    pub fn with_unavailable_retry_after(mut self, delay: Duration) -> Self {
        self.unavailable_retry_after = delay;
        self
    }

    pub fn max_body_bytes(self) -> usize {
        self.max_body_bytes
    }

    pub fn unavailable_retry_after(self) -> Duration {
        self.unavailable_retry_after
    }
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
            unavailable_retry_after: DEFAULT_UNAVAILABLE_RETRY_AFTER,
        }
    }
}
