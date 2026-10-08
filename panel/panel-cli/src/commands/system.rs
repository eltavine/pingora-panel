//! What runs, whether an upgrade can start, and the diagnostic bundle.

use super::{backups::private_file, gateway::protocols};
use crate::{
    client::{Api, CliError, Result},
    output::{text, Column, Format, Output},
};
use clap::Subcommand;
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    io::Write,
    path::PathBuf,
    time::{Duration, Instant},
};

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
    /// Checks a restored or upgraded installation: the gateway runs the
    /// revision expected, the certificate of every TLS profile is kept,
    /// the audit trail's chain is intact and every module serves its
    /// protocols; exits with 1 when a check fails.
    Verify {
        /// The revision the gateway must run.
        #[arg(long)]
        revision: Option<u64>,
        /// The hash of the snapshot the gateway must run.
        #[arg(long)]
        hash: Option<String>,
        /// Seconds to wait for the gateway to run it.
        #[arg(long, default_value_t = 120)]
        wait: u64,
    },
}

/// How often the gateway's status is read again while waiting.
const POLL: Duration = Duration::from_secs(1);

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
        SystemCommand::Verify {
            revision,
            hash,
            wait,
        } => verify(api, output, revision, hash, Duration::from_secs(wait)).await,
    }
}

fn check(name: &str, passed: bool, detail: String) -> Value {
    json!({ "name": name, "state": if passed { "pass" } else { "fail" }, "detail": detail })
}

/// Whether `status` is a ready gateway running what is expected.
fn runs(status: &Value, revision: Option<u64>, hash: Option<&str>) -> bool {
    status["ready"] == true
        && revision.is_none_or(|revision| status["active_revision_id"] == revision)
        && hash.is_none_or(|hash| status["active_hash"] == hash)
}

async fn verify(
    api: &Api,
    output: &Output,
    revision: Option<u64>,
    hash: Option<String>,
    wait: Duration,
) -> Result<()> {
    let deadline = Instant::now() + wait;
    let status = loop {
        let status = api
            .get("/api/v1/gateway/status", &[])
            .await
            .map(|reply| reply.body);
        match status {
            Ok(status) if runs(&status, revision, hash.as_deref()) => break Ok(status),
            status if Instant::now() >= deadline => break status,
            _ => tokio::time::sleep(POLL).await,
        }
    };
    let mut checks = vec![match status {
        Ok(status) => check(
            "gateway",
            runs(&status, revision, hash.as_deref()),
            format!(
                "ready: {}, revision {}, hash {}",
                text(&status["ready"]),
                text(&status["active_revision_id"]),
                text(&status["active_hash"])
            ),
        ),
        Err(error) => check("gateway", false, error.to_string()),
    }];

    let profiles = api.get("/api/v1/tls-profiles", &[]).await?.body;
    let certificates = api.get("/api/v1/certificates", &[]).await?.body;
    let kept: BTreeSet<&str> = certificates
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|certificate| certificate["id"].as_str())
        .collect();
    let named: Vec<(String, String)> = profiles
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|profile| {
            Some((
                text(&profile["id"]),
                profile["certificate_id"].as_str()?.to_owned(),
            ))
        })
        .collect();
    let missing: Vec<String> = named
        .iter()
        .filter(|(_, certificate)| !kept.contains(certificate.as_str()))
        .map(|(profile, certificate)| format!("{profile} names {certificate}"))
        .collect();
    checks.push(check(
        "certificates",
        missing.is_empty(),
        if missing.is_empty() {
            format!(
                "{} TLS profiles name kept certificates, {} kept",
                named.len(),
                kept.len()
            )
        } else {
            format!("not kept: {}", missing.join(", "))
        },
    ));

    let chain = api.get("/api/v1/audit-events/verify", &[]).await?.body;
    checks.push(check(
        "audit",
        chain["intact"] == true,
        if chain["intact"] == true {
            format!(
                "{} records chain to {}",
                text(&chain["checked"]),
                text(&chain["head_hash"])
            )
        } else {
            format!(
                "the chain breaks at record {}",
                text(&chain["first_mismatch"])
            )
        },
    ));

    let manifest = api.get("/api/v1/system/versions", &[]).await?.body;
    let modules = manifest["modules"].as_array().cloned().unwrap_or_default();
    let silent: Vec<String> = modules
        .iter()
        .filter(|module| module["protocols"].as_array().is_none_or(Vec::is_empty))
        .map(|module| text(&module["service"]))
        .collect();
    let problems: Vec<String> = manifest["problems"]
        .as_array()
        .into_iter()
        .flatten()
        .map(text)
        .collect();
    checks.push(check(
        "modules",
        !modules.is_empty() && silent.is_empty() && problems.is_empty(),
        if modules.is_empty() {
            "the service directory lists no modules".into()
        } else if !silent.is_empty() {
            format!("serving no protocols: {}", silent.join(", "))
        } else if !problems.is_empty() {
            problems.join("; ")
        } else {
            format!("{} modules serve their protocols", modules.len())
        },
    ));

    let verified = checks.iter().all(|check| check["state"] == "pass");
    let checks = Value::Array(checks);
    if output.format == Format::Json {
        output.json(&json!({ "verified": verified, "checks": checks }));
    } else {
        output.list(&checks, CHECKS);
    }
    if verified {
        return Ok(());
    }
    Err(CliError::Failed(
        "the installation does not verify".to_owned(),
    ))
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
        &json!({ "path": to.display().to_string() }),
    );
    Ok(())
}
