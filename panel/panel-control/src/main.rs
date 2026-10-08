#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        Some(panel_control::RESTORE_ARGUMENT) => panel_control::restore_main(&arguments[1..]),
        Some(panel_control::PREFLIGHT_ARGUMENT) => panel_control::preflight_main(&arguments[1..]),
        Some(panel_control::PROTOCOLS_ARGUMENT) => panel_control::protocols_main(),
        _ => panel_control_runtime::control_plane_main(&panel_control::modules()),
    }
}
