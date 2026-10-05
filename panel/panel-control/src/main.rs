#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.first().map(String::as_str) == Some(panel_control::RESTORE_ARGUMENT) {
        return panel_control::restore_main(&arguments[1..]);
    }
    panel_control_runtime::control_plane_main(&panel_control::modules())
}
