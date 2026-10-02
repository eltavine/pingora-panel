#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    panel_control_runtime::service_main(
        observability_service::default_addresses(),
        observability_service::process,
    )
}
