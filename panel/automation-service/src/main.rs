#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    panel_control_runtime::service_main(
        automation_service::default_addresses(),
        automation_service::process,
    )
}
