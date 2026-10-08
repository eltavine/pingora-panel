#![forbid(unsafe_code)]

//! The control plane: the public API with the configuration, automation,
//! observability, audit and plugins modules, in one process (ADR 0032).

pub mod preflight;
pub mod restore;

use panel_control_runtime::{Module, DATA_DIR_ENV, DEFAULT_DATA_DIR};
use panel_sqlite::SchemaMigration;
use preflight::{Finding, Verdict};
use std::{
    path::{Path, PathBuf},
    process::ExitCode,
};

/// The argument that restores databases from a backup instead of running.
pub const RESTORE_ARGUMENT: &str = "restore";
/// The argument that checks this release against the live databases and
/// the running release instead of running.
pub const PREFLIGHT_ARGUMENT: &str = "preflight";
/// The argument that prints the protocol revisions this release speaks.
pub const PROTOCOLS_ARGUMENT: &str = "protocols";
/// What the preflight exits with when a migration contracts a schema.
pub const CONTRACTS: u8 = 3;

/// The modules in the order they start; they stop in reverse, so the API
/// stops taking requests first and plugins stop after the modules that
/// call them.
pub fn modules() -> [Module; 6] {
    [
        Module::new(
            audit_service::SERVICE,
            audit_service::default_addresses(),
            audit_service::process,
        ),
        Module::new(
            plugins_service::SERVICE,
            plugins_service::default_addresses(),
            plugins_service::process,
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
pub fn databases() -> [(&'static str, &'static [SchemaMigration]); 6] {
    [
        (audit_service::MODULE, audit_service::MIGRATIONS),
        (plugins_service::MODULE, plugins_service::MIGRATIONS),
        (config_service::MODULE, config_service::MIGRATIONS),
        (automation_service::MODULE, automation_service::MIGRATIONS),
        (
            observability_service::MODULE,
            observability_service::MIGRATIONS,
        ),
        (panel_api_server::MODULE, panel_api_server::MIGRATIONS),
    ]
}

fn default_data_directory() -> PathBuf {
    std::env::var_os(DATA_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_DATA_DIR))
}

/// The value of each `--name VALUE` pair `names` allows, or `None` when
/// the arguments hold anything else.
fn options<'a>(arguments: &'a [String], names: &[&str]) -> Option<Vec<(&'a str, &'a str)>> {
    let mut found = Vec::new();
    let mut rest = arguments.iter();
    while let Some(name) = rest.next() {
        let value = rest.next()?;
        if !names.contains(&name.as_str()) {
            return None;
        }
        found.push((name.as_str(), value.as_str()));
    }
    Some(found)
}

fn runtime(command: &str) -> Option<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            eprintln!("panel-control {command}: cannot start the async runtime: {error}")
        })
        .ok()
}

/// `panel-control restore ARCHIVE [--data-dir DIRECTORY] [--sites
/// DIRECTORY]`: installs the databases of a backup, and its sites when a
/// directory for them is named, while the control plane is stopped. The
/// last lines name the revision the configuration wants the gateway to run
/// as `desired_revision=` and `desired_hash=`.
pub fn restore_main(arguments: &[String]) -> ExitCode {
    let usage = || {
        eprintln!(
            "usage: panel-control restore ARCHIVE [--data-dir DIRECTORY] [--sites DIRECTORY]"
        );
        ExitCode::from(2)
    };
    let Some((archive, rest)) = arguments.split_first() else {
        return usage();
    };
    let Some(options) = options(rest, &["--data-dir", "--sites"]) else {
        return usage();
    };
    let archive = PathBuf::from(archive);
    let mut data_directory = default_data_directory();
    let mut sites = None;
    for (name, value) in options {
        match name {
            "--data-dir" => data_directory = PathBuf::from(value),
            _ => sites = Some(PathBuf::from(value)),
        }
    }
    let Some(runtime) = runtime("restore") else {
        return ExitCode::FAILURE;
    };
    let restored = restore::stopped(modules().iter().map(Module::ops_address))
        .and_then(|()| runtime.block_on(restore::restore(&archive, &data_directory, &databases())))
        .and_then(|installed| {
            let sites = sites
                .map(|sites| restore::restore_sites(&archive, &sites))
                .transpose()?
                .flatten();
            let desired = runtime.block_on(restore::desired(&data_directory))?;
            Ok((installed, sites, desired))
        });
    match restored {
        Ok((installed, sites, desired)) => {
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
            if let Some(sites) = sites {
                println!("installed the sites: {} files", sites.files);
            }
            println!("start the control plane; it brings each database up to this release");
            if let Some((revision, hash)) = desired {
                println!("desired_revision={revision}");
                println!("desired_hash={hash}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("panel-control restore: {}", error.message);
            ExitCode::FAILURE
        }
    }
}

/// `panel-control protocols`: the protocol revisions every component of
/// this release speaks, as JSON, for the preflight of the release after it.
pub fn protocols_main() -> ExitCode {
    let revisions: Vec<preflight::Revisions> =
        panel_contracts::PROTOCOLS.iter().map(Into::into).collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&revisions).expect("revisions serialize")
    );
    ExitCode::SUCCESS
}

/// `panel-control preflight [--data-dir DIRECTORY] [--peers FILE]`: what
/// upgrading to this release would do, changing nothing: the schema each
/// database migrates from and to, migrated on a copy, and, with `--peers`,
/// whether this release speaks the protocols of the running one. It exits
/// with 1 when something holds the upgrade back, and with [`CONTRACTS`]
/// when a migration contracts a schema, which only the backup undoes.
pub fn preflight_main(arguments: &[String]) -> ExitCode {
    let Some(options) = options(arguments, &["--data-dir", "--peers"]) else {
        eprintln!("usage: panel-control preflight [--data-dir DIRECTORY] [--peers FILE]");
        return ExitCode::from(2);
    };
    let mut data_directory = default_data_directory();
    let mut findings = Vec::new();
    for (name, value) in options {
        match name {
            "--data-dir" => data_directory = PathBuf::from(value),
            _ => match std::fs::read_to_string(value)
                .map_err(|error| error.to_string())
                .and_then(|text| preflight::peers(&text).map_err(|error| error.message))
            {
                Ok(peers) => {
                    findings.extend(preflight::protocol_findings(
                        panel_contracts::PROTOCOLS,
                        &peers,
                    ));
                }
                Err(error) => findings.push(Finding::new(Verdict::Fail, "peers", error)),
            },
        }
    }
    let Some(runtime) = runtime("preflight") else {
        return ExitCode::FAILURE;
    };
    let scratch =
        std::env::temp_dir().join(format!("pingora-panel-preflight-{}", std::process::id()));
    let (checked, contracts) = match std::fs::create_dir_all(&scratch) {
        Ok(()) => runtime.block_on(preflight::databases(
            &data_directory,
            &scratch,
            &databases(),
        )),
        Err(error) => (
            vec![Finding::new(
                Verdict::Fail,
                "scratch",
                format!("{} cannot be created: {error}", scratch.display()),
            )],
            false,
        ),
    };
    let _ = std::fs::remove_dir_all(&scratch);
    findings.extend(checked);
    report(&findings, contracts, &data_directory)
}

fn report(findings: &[Finding], contracts: bool, data_directory: &Path) -> ExitCode {
    println!("checked the databases in {}", data_directory.display());
    for finding in findings {
        println!("{finding}");
    }
    if findings
        .iter()
        .any(|finding| finding.verdict == Verdict::Fail)
    {
        ExitCode::FAILURE
    } else if contracts {
        ExitCode::from(CONTRACTS)
    } else {
        ExitCode::SUCCESS
    }
}
