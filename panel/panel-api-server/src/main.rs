#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    panel_control_runtime::service_main(
        panel_api_server::default_addresses(),
        panel_api_server::process,
    )
}
