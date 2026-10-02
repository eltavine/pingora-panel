#![forbid(unsafe_code)]

use panel_bootstrap::Plan;
use panel_service::{init_logging, Environment};
use std::{process::ExitCode, time::Duration};

const ATTEMPTS: u32 = 30;
const RETRY_DELAY: Duration = Duration::from_secs(2);

fn main() -> ExitCode {
    init_logging();
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
