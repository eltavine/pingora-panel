#![forbid(unsafe_code)]

use std::process::ExitCode;

#[cfg(unix)]
fn main() -> ExitCode {
    use ops_agent::AgentConfig;
    use panel_service::{init_logging, shutdown_signal, Environment};

    init_logging();
    let config = match AgentConfig::read(&mut Environment::process()) {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(error_code = %error.code, error = %error.message, "invalid settings");
            return ExitCode::FAILURE;
        }
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!(%error, "cannot start the async runtime");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(ops_agent::serve(config, shutdown_signal())) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(error_code = %error.code, error = %error.message, "the agent stopped");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(unix))]
fn main() -> ExitCode {
    eprintln!("ops-agent runs on Unix hosts only");
    ExitCode::FAILURE
}
