#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    panel_control_runtime::service_main(audit_service::default_addresses(), audit_service::process)
}
