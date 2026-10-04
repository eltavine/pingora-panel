use crate::{
    ControlPlaneProcess, DefaultAddresses, ProcessSettings, HEALTHCHECK_ARGUMENT, OPS_ADDRESS_ENV,
};
use panel_errors::Result;
use panel_service::{init_logging, probe_http, shutdown_signal, Environment, READINESS_PATH};
use std::{process::ExitCode, time::Duration};

const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// The entry point of a control-plane service binary.
///
/// Started with `healthcheck`, it probes the readiness endpoint of the
/// instance configured by the same environment and exits accordingly.
/// Otherwise it builds the process from the environment, runs it until
/// SIGINT or SIGTERM and stops it gracefully.
pub fn service_main(
    defaults: DefaultAddresses,
    build: impl FnOnce(&mut Environment<'static>, ProcessSettings) -> Result<ControlPlaneProcess>,
) -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("cannot start the async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    if std::env::args().nth(1).as_deref() == Some(HEALTHCHECK_ARGUMENT) {
        let address = Environment::process().socket_addr(OPS_ADDRESS_ENV, defaults.ops);
        let ready = match address {
            Ok(address) => runtime.block_on(probe_http(address, READINESS_PATH, PROBE_TIMEOUT)),
            Err(_) => false,
        };
        return if ready {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    init_logging();
    let result = runtime.block_on(async {
        let mut env = Environment::process();
        let settings = ProcessSettings::read(&mut env, defaults)?;
        let process = build(&mut env, settings)?.start().await?;
        process.run_until(shutdown_signal()).await;
        Ok::<_, panel_errors::PanelError>(())
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(error_code = %error.code, error = %error.message, "service failed to start");
            ExitCode::FAILURE
        }
    }
}
