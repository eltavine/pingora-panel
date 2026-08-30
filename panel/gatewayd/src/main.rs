use gatewayd::{initialize_observability, serve_gatewayd, GatewaydConfig, GatewaydError};

fn main() -> Result<(), GatewaydError> {
    initialize_observability();
    let config = GatewaydConfig::from_environment()?;
    let executor = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(config.worker_count().get() as usize)
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
