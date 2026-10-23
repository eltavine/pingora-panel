#![forbid(unsafe_code)]

use gatewayd::{initialize_observability, serve_gatewayd, GatewaydConfig, GatewaydError};

#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static ALLOCATOR: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/// Management traffic is light; proxied traffic runs on the data plane's
/// own workers, sized by the configured worker count.
const MANAGEMENT_THREADS: usize = 2;

fn main() -> Result<(), GatewaydError> {
    initialize_observability();
    let config = GatewaydConfig::from_environment()?;
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(MANAGEMENT_THREADS)
        .enable_all()
        .build()?;
    executor.block_on(async {
        // Register handlers before startup can publish readiness.
        let shutdown = shutdown_signal()?;
        serve_gatewayd(config, shutdown).await
    })
}

#[cfg(unix)]
fn shutdown_signal() -> std::io::Result<impl std::future::Future<Output = ()>> {
    use tokio::signal::unix::{signal, SignalKind};

    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    Ok(async move {
        tokio::select! {
            _ = interrupt.recv() => {}
            _ = terminate.recv() => {}
        }
    })
}

#[cfg(not(unix))]
fn shutdown_signal() -> std::io::Result<impl std::future::Future<Output = ()>> {
    Ok(async {
        let _ = tokio::signal::ctrl_c().await;
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn signals_are_captured_before_the_shutdown_future_is_polled() {
        let shutdown = shutdown_signal().unwrap();
        let status = std::process::Command::new("kill")
            .arg("-TERM")
            .arg(std::process::id().to_string())
            .status()
            .unwrap();
        assert!(status.success());
        tokio::time::timeout(std::time::Duration::from_secs(5), shutdown)
            .await
            .expect("SIGTERM must remain pending until shutdown starts polling");
    }
}
