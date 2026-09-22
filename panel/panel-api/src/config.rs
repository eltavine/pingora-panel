use panel_errors::PanelError;

const DEFAULT_MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

/// Resource policy for the public HTTP adapter.
///
/// Private fields plus validated construction allow future limits to be added
/// without exposing a public struct layout to callers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApiConfig {
    max_body_bytes: usize,
}

impl ApiConfig {
    pub fn new(max_body_bytes: usize) -> Result<Self, PanelError> {
        if max_body_bytes == 0 {
            return Err(PanelError::invalid_argument(
                "API request body limit must be non-zero",
            ));
        }
        Ok(Self { max_body_bytes })
    }

    pub fn max_body_bytes(self) -> usize {
        self.max_body_bytes
    }
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            max_body_bytes: DEFAULT_MAX_BODY_BYTES,
        }
    }
}
