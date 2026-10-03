#![forbid(unsafe_code)]

use gatewayd::{initialize_observability, serve_gatewayd, GatewaydConfig, GatewaydError};

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
    executor.block_on(serve_gatewayd(config, shutdown_signal()))
}

#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};

    let Ok(mut terminate) = signal(SignalKind::terminate()) else {
        let _ = tokio::signal::ctrl_c().await;
        return;
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
