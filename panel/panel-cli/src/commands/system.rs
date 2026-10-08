//! What runs, whether an upgrade can start, and the diagnostic bundle.

use super::{backups::private_file, gateway::protocols};
use crate::{
    client::{Api, CliError, Result},
    output::{text, Column, Format, Output},
};
use clap::Subcommand;
use serde_json::Value;
use std::{io::Write, path::PathBuf};

#[derive(Subcommand)]
pub(crate) enum SystemCommand {
    /// The versions of the release, each module, the gateway, the host
    /// agent and the deployed images.
    Versions,
    /// Whether an upgrade can start now; exits with 1 when a check fails.
    Preflight,
    /// Downloads the diagnostic bundle: versions, health, recent failures
    /// and audit events, with every secret removed.
    Diagnostics {
        /// Where to write it, `pingora-panel-diagnostics-TIME.json` by
        /// default; `-` for standard output.
        #[arg(long)]
        to: Option<PathBuf>,
    },
}

const VERSIONS: &[Column] = &[
    ("Release", |manifest| {
        format!(
            "{} ({})",
            text(&manifest["release"]),
            text(&manifest["commit"])
        )
    }),
    ("API", |manifest| text(&manifest["api"])),
    ("Language", |manifest| text(&manifest["language"])),
    ("IR schema", |manifest| text(&manifest["ir_schema"])),
    ("Gateway", |manifest| match &manifest["gateway"] {
        Value::Null => "-".into(),
        gateway => format!(
            "{} · engine {} · adapter {}",
            text(&gateway["gateway"]),
            text(&gateway["engine"]),
            text(&gateway["adapter"])
        ),
    }),
    ("Agent", |manifest| match &manifest["agent"] {
        Value::Null => "-".into(),
        agent => format!(
            "{} · protocol {} · {}",
            text(&agent["build"]),
            text(&agent["protocol"]),
            text(&agent["hostname"])
        ),
    }),
    ("Deployed", |manifest| match &manifest["deployment"] {
        Value::Null => "-".into(),
        deployment => format!(
            "{} by {} at {}{}",
            text(&deployment["action"]),
            text(&deployment["engine"]),
            text(&deployment["changed_at"]),
            deployment["previous"]
                .as_str()
                .map(|previous| format!(", from {previous}"))
                .unwrap_or_default()
        ),
    }),
];

const MODULES: &[Column] = &[
    ("MODULE", |module| text(&module["service"])),
    ("BUILD", |module| text(&module["build_version"])),
    ("SCHEMA", |module| text(&module["schema_version"])),
    ("PROTOCOLS", protocols),
];

const IMAGES: &[Column] = &[
    ("SERVICE", |image| text(&image["service"])),
    ("IMAGE", |image| text(&image["image"])),
    ("DIGEST", |image| text(&image["digest"])),
];

const CHECKS: &[Column] = &[
    ("CHECK", |check| text(&check["name"])),
    ("STATE", |check| text(&check["state"])),
    ("DETAIL", |check| text(&check["detail"])),
];

pub async fn run(api: &Api, output: &Output, command: SystemCommand) -> Result<()> {
    match command {
        SystemCommand::Versions => versions(api, output).await,
        SystemCommand::Preflight => preflight(api, output).await,
        SystemCommand::Diagnostics { to } => diagnostics(api, output, to).await,
    }
}

async fn versions(api: &Api, output: &Output) -> Result<()> {
    let manifest = api.get("/api/v1/system/versions", &[]).await?.body;
    if output.format == Format::Json {
        output.json(&manifest);
        return Ok(());
    }
    if output.quiet {
        return Ok(());
    }
    output.item(&manifest, VERSIONS);
    println!();
    output.list(&manifest["modules"], MODULES);
    if let Some(images) = manifest["deployment"]["images"].as_array() {
        if !images.is_empty() {
            println!();
            output.list(&manifest["deployment"]["images"], IMAGES);
        }
    }
    for problem in manifest["problems"].as_array().into_iter().flatten() {
        eprintln!("{}", text(problem));
    }
    Ok(())
}

async fn preflight(api: &Api, output: &Output) -> Result<()> {
    let readiness = api.get("/api/v1/system/preflight", &[]).await?.body;
    if output.format == Format::Json {
        output.json(&readiness);
    } else {
        output.list(&readiness["checks"], CHECKS);
    }
    if readiness["ready"] == true {
        if output.format == Format::Table && !output.quiet {
            println!("\nan upgrade can start");
        }
        return Ok(());
    }
    let failed: Vec<String> = readiness["checks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|check| check["state"] == "fail")
        .map(|check| text(&check["name"]))
        .collect();
    Err(CliError::Failed(format!(
        "an upgrade cannot start: {} failed",
        failed.join(", ")
    )))
}

async fn diagnostics(api: &Api, output: &Output, to: Option<PathBuf>) -> Result<()> {
    let bundle = api.get("/api/v1/system/diagnostics", &[]).await?.body;
    let document = serde_json::to_string_pretty(&bundle).expect("JSON values serialize");
    let to = to.unwrap_or_else(|| {
        let stamp: String = text(&bundle["generated_at"])
            .chars()
            .filter(|character| !matches!(character, '-' | ':'))
            .collect();
        PathBuf::from(format!("pingora-panel-diagnostics-{stamp}.json"))
    });
    if to.as_os_str() == "-" {
        println!("{document}");
        return Ok(());
    }
    let mut file = private_file(&to)?;
    file.write_all(document.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .map_err(|error| CliError::Failed(format!("cannot write {}: {error}", to.display())))?;
    output.done(
        &format!("wrote the diagnostic bundle to {}", to.display()),
        &serde_json::json!({ "path": to.display().to_string() }),
    );
    Ok(())
}
