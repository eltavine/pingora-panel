use backon::{BackoffBuilder, ExponentialBuilder};
use panel_errors::Result;
use std::{future::Future, time::Duration};
use tokio_util::sync::CancellationToken;

/// Repeats `operation` with jittered exponential backoff until it succeeds
/// or the process stops, logging the first failure and recovery.
pub(crate) async fn until_success<T, F, Fut>(
    what: &'static str,
    cancel: &CancellationToken,
    mut operation: F,
) -> Option<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T>>,
{
    let mut delays = ExponentialBuilder::default()
        .with_min_delay(Duration::from_millis(200))
        .with_max_delay(Duration::from_secs(10))
        .with_jitter()
        .without_max_times()
        .build();
    let mut failed = false;
    loop {
        let attempt = tokio::select! {
            () = cancel.cancelled() => return None,
            attempt = operation() => attempt,
        };
        match attempt {
            Ok(value) => {
                if failed {
                    tracing::info!(task = what, "recovered");
                }
                return Some(value);
            }
            Err(error) => {
                if !failed {
                    tracing::warn!(
                        task = what,
                        error_code = %error.code,
                        error = %error.message,
                        "failed; retrying with backoff"
                    );
                }
                failed = true;
                let delay = delays.next().unwrap_or(Duration::from_secs(10));
                tokio::select! {
                    () = cancel.cancelled() => return None,
                    () = tokio::time::sleep(delay) => {}
                }
            }
        }
    }
}
