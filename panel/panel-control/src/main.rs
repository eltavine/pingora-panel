#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    panel_control_runtime::control_plane_main(&panel_control::modules())
}
