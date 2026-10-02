#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    panel_control_runtime::service_main(
        config_service::default_addresses(),
        config_service::process,
    )
}
