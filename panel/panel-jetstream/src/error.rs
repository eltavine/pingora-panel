use panel_errors::PanelError;

/// Maps a broker failure onto the stable error model. Broker errors are
/// transient from the caller's perspective, so they are retryable.
pub(crate) fn broker_error(
    operation: &str,
    error: impl std::error::Error + Send + Sync + 'static,
) -> PanelError {
    PanelError::storage_unavailable(format!("event broker {operation} failed: {error}"))
        .with_source(error)
}
