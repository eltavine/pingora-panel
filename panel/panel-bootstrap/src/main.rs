#![forbid(unsafe_code)]

use panel_bootstrap::{PkiPlan, Plan};
use panel_service::{init_logging, shutdown_signal, Environment};
use std::{process::ExitCode, time::Duration};

const ATTEMPTS: u32 = 30;
const RETRY_DELAY: Duration = Duration::from_secs(2);

fn main() -> ExitCode {
    init_logging();
    let mut arguments = std::env::args().skip(1);
    if arguments.next().as_deref() == Some("pki") {
        return pki(arguments.next().as_deref() == Some("--once"));
    }
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("cannot start the async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    let plan = match Plan::read(&mut Environment::process()) {
        Ok(plan) => plan,
        Err(error) => {
            tracing::error!(error_code = %error.code, error = %error.message, "invalid settings");
            return ExitCode::FAILURE;
        }
    };
    runtime.block_on(async {
        for attempt in 1..=ATTEMPTS {
            match plan.apply().await {
                Ok(()) => return ExitCode::SUCCESS,
                Err(error) if error.retryable && attempt < ATTEMPTS => {
                    tracing::warn!(attempt, error_code = %error.code, "dependencies not ready; retrying");
                    tokio::time::sleep(RETRY_DELAY).await;
                }
                Err(error) => {
                    tracing::error!(error_code = %error.code, error = %error.message, "bootstrap failed");
                    return ExitCode::FAILURE;
                }
            }
        }
        ExitCode::FAILURE
    })
}

/// Creates the certificate authority and issues due credentials, once or
/// until SIGINT or SIGTERM.
fn pki(once: bool) -> ExitCode {
    let plan = match PkiPlan::read(&mut Environment::process()) {
        Ok(plan) => plan,
        Err(error) => {
            tracing::error!(error_code = %error.code, error = %error.message, "invalid settings");
            return ExitCode::FAILURE;
        }
    };
    if once {
        return match plan.apply() {
            Ok(_) => ExitCode::SUCCESS,
            Err(error) => {
                tracing::error!(error_code = %error.code, error = %error.message, "credential issuance failed");
                ExitCode::FAILURE
            }
        };
    }
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return ExitCode::FAILURE;
    };
    runtime.block_on(async {
        let shutdown = tokio_util::sync::CancellationToken::new();
        let stop = shutdown.clone();
        tokio::spawn(async move {
            shutdown_signal().await;
            stop.cancel();
        });
        plan.maintain(shutdown).await;
    });
    ExitCode::SUCCESS
}
