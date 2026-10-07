#![forbid(unsafe_code)]

//! `pingora-panel-reference-plugin`: served by the panel's plugin host, or
//! `pingora-panel-reference-plugin package <plugins directory> [<version>]`
//! to install a copy of itself there, signed by a new key whose public key
//! it prints for the host to trust.

use plugin_reference::{package, services};
use std::{path::PathBuf, process::ExitCode};

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments.first().map(String::as_str) == Some("package") {
        return install(&arguments[1..]);
    }
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("cannot start the async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    let served = runtime.block_on(async {
        let state = services::State::new(plugin_sdk::data_dir());
        let plugin = plugin_sdk::Plugin::from_manifest_file("plugin.json")?;
        services::register(plugin, state).serve().await
    });
    match served {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn install(arguments: &[String]) -> ExitCode {
    let (root, version) = match arguments {
        [root] => (root, env!("CARGO_PKG_VERSION")),
        [root, version] => (root, version.as_str()),
        _ => {
            eprintln!(
                "usage: pingora-panel-reference-plugin package <plugins directory> [<version>]"
            );
            return ExitCode::from(2);
        }
    };
    let publisher = package::Publisher::generate();
    let installed = std::env::current_exe().and_then(|executable| {
        package::install(
            &PathBuf::from(root),
            &executable,
            &package::manifest(version),
            &publisher,
        )
    });
    match installed {
        Ok(directory) => {
            eprintln!("installed {}", directory.display());
            println!("{}", publisher.public_key());
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("cannot install the plugin: {error}");
            ExitCode::FAILURE
        }
    }
}
