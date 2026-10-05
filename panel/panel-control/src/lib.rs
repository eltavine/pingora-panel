#![forbid(unsafe_code)]

//! The control plane: the public API with the configuration, automation,
//! observability and audit modules, in one process (ADR 0032).

pub mod restore;

use panel_control_runtime::{Module, DATA_DIR_ENV, DEFAULT_DATA_DIR};
use panel_sqlite::SchemaMigration;
use std::{path::PathBuf, process::ExitCode};

/// The argument that restores databases from a backup instead of running.
pub const RESTORE_ARGUMENT: &str = "restore";

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

/// Each module's database by module name, with the migrations that bring it
/// up to this release.
pub fn databases() -> [(&'static str, &'static [SchemaMigration]); 5] {
    [
        (audit_service::MODULE, audit_service::MIGRATIONS),
        (config_service::MODULE, config_service::MIGRATIONS),
        (automation_service::MODULE, automation_service::MIGRATIONS),
        (
            observability_service::MODULE,
            observability_service::MIGRATIONS,
        ),
        (panel_api_server::MODULE, panel_api_server::MIGRATIONS),
    ]
}

/// `panel-control restore ARCHIVE [--data-dir DIRECTORY]`: installs the
/// databases of a backup while the control plane is stopped.
pub fn restore_main(arguments: &[String]) -> ExitCode {
    let (archive, data_directory) = match arguments {
        [archive] => (
            PathBuf::from(archive),
            std::env::var_os(DATA_DIR_ENV)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(DEFAULT_DATA_DIR)),
        ),
        [archive, flag, directory] if flag == "--data-dir" => {
            (PathBuf::from(archive), PathBuf::from(directory))
        }
        _ => {
            eprintln!("usage: panel-control restore ARCHIVE [--data-dir DIRECTORY]");
            return ExitCode::from(2);
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("panel-control restore: cannot start the async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    let restored = restore::stopped(modules().iter().map(Module::ops_address))
        .and_then(|()| runtime.block_on(restore::restore(&archive, &data_directory, &databases())));
    match restored {
        Ok(installed) => {
            for database in installed {
                match database.replaced {
                    Some(kept) => println!(
                        "installed the {} database, keeping the one it replaced as {}",
                        database.module,
                        kept.display()
                    ),
                    None => println!("installed the {} database", database.module),
                }
            }
            println!("start the control plane; it brings each database up to this release");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("panel-control restore: {}", error.message);
            ExitCode::FAILURE
        }
    }
}
