#![forbid(unsafe_code)]

//! The control plane: the public API with the configuration, automation,
//! observability and audit modules, in one process (ADR 0032).

use panel_control_runtime::Module;

/// The modules in the order they start; they stop in reverse, so the API
/// stops taking requests first.
pub fn modules() -> [Module; 5] {
    [
        Module::new(
            audit_service::SERVICE,
            audit_service::default_addresses(),
            audit_service::process,
        ),
        Module::new(
            config_service::SERVICE,
            config_service::default_addresses(),
            config_service::process,
        ),
        Module::new(
            automation_service::SERVICE,
            automation_service::default_addresses(),
            automation_service::process,
        ),
        Module::new(
            observability_service::SERVICE,
            observability_service::default_addresses(),
            observability_service::process,
        ),
        Module::new(
            panel_api_server::SERVICE,
            panel_api_server::default_addresses(),
            panel_api_server::process,
        ),
    ]
}
