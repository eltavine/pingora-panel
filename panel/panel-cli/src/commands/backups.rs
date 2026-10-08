//! Backups of the installation (ADR 0035): archives of the databases and
//! the sites' directory, taken in the background and checked against their
//! digests when downloaded.

use crate::{
    client::{Api, CliError, Result},
    output::{bytes, text, Column, Format, Output},
};
use base64::Engine;
use clap::{Subcommand, ValueEnum};
use reqwest::Method;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs::File, io::Write, path::PathBuf, time::Duration};

/// How long downloading or restoring a large archive may take.
const LASTING: Duration = Duration::from_secs(60 * 60);
/// How often a backup being taken is looked at while waiting for it.
const POLL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Content {
    /// The configuration's database, with the draft and the active revision.
    Configuration,
    /// Certificates with their keys sealed, ACME accounts and DNS providers.
    Certificates,
    /// Every module's database.
    Databases,
    /// The sites' directory, or one directory below it with --site.
    Sites,
}

impl Content {
    fn as_str(self) -> &'static str {
        match self {
            Self::Configuration => "configuration",
            Self::Certificates => "certificates",
            Self::Databases => "databases",
            Self::Sites => "sites",
        }
    }
}

#[derive(Subcommand)]
pub(crate) enum BackupCommand {
    /// Backups, newest first.
    List,
    /// A backup and how far taking it got.
    Show { id: String },
    /// Takes a backup in the background.
    Create {
        /// What it holds, such as `configuration,sites`.
        #[arg(long = "with", value_enum, value_delimiter = ',', required = true)]
        contents: Vec<Content>,
        /// With the sites, the directory below the sites' directory to hold.
        #[arg(long)]
        site: Option<String>,
        /// Waits until it is taken, failing when it fails.
        #[arg(long)]
        wait: bool,
    },
    /// Downloads a backup's archive, checked against its digest.
    Download {
        id: String,
        /// Where to write it, `pingora-panel-backup-ID.tar.zst` by default;
        /// `-` for standard output.
        #[arg(long)]
        to: Option<PathBuf>,
    },
    /// Restores a directory of the sites, or the configuration as the draft.
    Restore {
        id: String,
        /// The directory below the sites' directory to replace.
        #[arg(
            long,
            required_unless_present = "configuration",
            conflicts_with = "configuration"
        )]
        sites: Option<String>,
        /// Saves the backup's active revision as the draft.
        #[arg(long)]
        configuration: bool,
        /// With --configuration, refuse if the draft changed since this
        /// version.
        #[arg(long, requires = "configuration")]
        expected_version: Option<u64>,
    },
    /// Removes a backup and its archive.
    Rm {
        id: String,
        /// Confirms the removal.
        #[arg(long)]
        yes: bool,
    },
    /// Copies a taken backup's archive to a plugin's backup target.
    Copy {
        id: String,
        /// The plugin whose backup target keeps the copy.
        #[arg(long = "to")]
        target: String,
    },
    /// The archives a plugin's backup target keeps.
    #[command(subcommand)]
    Target(TargetCommand),
}

#[derive(Subcommand)]
pub(crate) enum TargetCommand {
    /// The archives a plugin's backup target keeps.
    Ls { target: String },
    /// Fetches an archive and keeps it as a backup once it checks against
    /// its manifest.
    Import { target: String, name: String },
    /// Removes an archive from a plugin's backup target.
    Rm {
        target: String,
        name: String,
        /// Confirms the removal.
        #[arg(long)]
        yes: bool,
    },
}

const ARCHIVES: &[Column] = &[
    ("NAME", |archive| text(&archive["name"])),
    ("SIZE", |archive| bytes(&archive["size_bytes"])),
    ("CREATED", |archive| text(&archive["created_at"])),
    ("SHA-256", |archive| text(&archive["sha256"])),
];

fn target_path(target: &str) -> String {
    format!("/api/v1/backup-targets/{target}/archives")
}

const BACKUPS: &[Column] = &[
    ("ID", |backup| text(&backup["id"])),
    ("STATE", |backup| text(&backup["state"])),
    ("HOLDS", contents),
    ("SIZE", |backup| bytes(&backup["size_bytes"])),
    ("REQUESTED", |backup| text(&backup["requested_at"])),
    ("BY", |backup| text(&backup["requested_by"])),
];

const BACKUP: &[Column] = &[
    ("ID", |backup| text(&backup["id"])),
    ("State", |backup| text(&backup["state"])),
    ("Holds", contents),
    ("Site", |backup| text(&backup["site_path"])),
    ("Size", |backup| bytes(&backup["size_bytes"])),
    ("Files", |backup| text(&backup["files"])),
    ("SHA-256", |backup| text(&backup["sha256"])),
    ("Requested", |backup| text(&backup["requested_at"])),
    ("By", |backup| text(&backup["requested_by"])),
    ("Finished", |backup| text(&backup["finished_at"])),
    ("Failure", |backup| text(&backup["failure"]["message"])),
    ("Version", |backup| text(&backup["product_version"])),
];

fn contents(backup: &Value) -> String {
    backup["contents"]
        .as_array()
        .map(|contents| contents.iter().map(text).collect::<Vec<_>>().join(","))
        .unwrap_or_default()
}

fn path(id: &str) -> String {
    format!("/api/v1/backups/{id}")
}

pub(crate) async fn run(api: &Api, output: &Output, command: BackupCommand) -> Result<()> {
    match command {
        BackupCommand::List => {
            let listed = api.get("/api/v1/backups", &[]).await?.body;
            if output.format == Format::Json {
                output.json(&listed);
            } else if listed["backups"].as_array().is_some_and(Vec::is_empty) {
                if !output.quiet {
                    eprintln!("there are no backups");
                }
            } else {
                output.list(&listed["backups"], BACKUPS);
            }
        }
        BackupCommand::Show { id } => {
            let backup = api.get(&path(&id), &[]).await?.body;
            output.item(&backup, BACKUP);
        }
        BackupCommand::Create {
            contents,
            site,
            wait,
        } => {
            let body = json!({
                "contents": contents.iter().map(|content| content.as_str()).collect::<Vec<_>>(),
                "site_path": site,
            });
            let mut backup = api
                .change(Method::POST, "/api/v1/backups", Some(&body), None)
                .await?
                .body;
            if wait {
                let id = text(&backup["id"]);
                while matches!(backup["state"].as_str(), Some("pending" | "running")) {
                    tokio::time::sleep(POLL).await;
                    backup = api.get(&path(&id), &[]).await?.body;
                }
                if backup["state"] == "failed" {
                    return Err(CliError::Failed(format!(
                        "the backup failed: {}",
                        text(&backup["failure"]["message"])
                    )));
                }
                output.done(
                    &format!(
                        "Took backup {id}: {} files, {}",
                        text(&backup["files"]),
                        bytes(&backup["size_bytes"])
                    ),
                    &backup,
                );
            } else {
                output.done(&format!("Taking backup {}", text(&backup["id"])), &backup);
            }
        }
        BackupCommand::Download { id, to } => {
            let to =
                to.unwrap_or_else(|| PathBuf::from(format!("pingora-panel-backup-{id}.tar.zst")));
            download(api, &id, to, output).await?;
        }
        BackupCommand::Restore {
            id,
            sites,
            configuration,
            expected_version,
        } => {
            let (body, if_match) = match sites {
                Some(site) if !configuration => {
                    (json!({ "target": "sites", "site_path": site }), None)
                }
                _ => (
                    json!({ "target": "configuration" }),
                    expected_version.map(|version| format!("\"draft-{version}\"")),
                ),
            };
            let restored = api
                .change_lasting(
                    Method::POST,
                    &format!("{}/restores", path(&id)),
                    Some(&body),
                    if_match.as_deref(),
                    LASTING,
                )
                .await?
                .body;
            let message = if restored["target"] == "sites" {
                format!(
                    "Restored {} from backup {id}: {} files, {}",
                    text(&restored["site_path"]),
                    text(&restored["files"]),
                    bytes(&restored["bytes"])
                )
            } else {
                format!(
                    "Saved the configuration of backup {id} as draft {}; review and apply it",
                    text(&restored["draft_version"])
                )
            };
            output.done(&message, &restored);
        }
        BackupCommand::Rm { id, yes } => {
            if !yes {
                return Err(CliError::Usage(format!(
                    "removing backup {id} cannot be undone; pass --yes"
                )));
            }
            api.change(Method::DELETE, &path(&id), None, None).await?;
            output.done(&format!("Removed backup {id}"), &json!({ "id": id }));
        }
        BackupCommand::Copy { id, target } => {
            let copy = api
                .change_lasting(
                    Method::POST,
                    &format!("{}/copies", path(&id)),
                    Some(&json!({ "target": target })),
                    None,
                    LASTING,
                )
                .await?
                .body;
            output.done(
                &format!(
                    "Copied backup {id} to {target} as {} ({})",
                    text(&copy["name"]),
                    bytes(&copy["size_bytes"])
                ),
                &copy,
            );
        }
        BackupCommand::Target(TargetCommand::Ls { target }) => {
            let archives = api.get(&target_path(&target), &[]).await?.body;
            output.list(&archives, ARCHIVES);
        }
        BackupCommand::Target(TargetCommand::Import { target, name }) => {
            let backup = api
                .change_lasting(
                    Method::POST,
                    &format!("{}/{name}/imports", target_path(&target)),
                    None,
                    None,
                    LASTING,
                )
                .await?
                .body;
            output.done(
                &format!(
                    "Kept {name} from {target} as backup {}: {} files",
                    text(&backup["id"]),
                    text(&backup["files"])
                ),
                &backup,
            );
        }
        BackupCommand::Target(TargetCommand::Rm { target, name, yes }) => {
            if !yes {
                return Err(CliError::Usage(format!(
                    "removing {name} from {target} cannot be undone; pass --yes"
                )));
            }
            api.change(
                Method::DELETE,
                &format!("{}/{name}", target_path(&target)),
                None,
                None,
            )
            .await?;
            output.done(
                &format!("Removed {name} from {target}"),
                &json!({ "target": target, "name": name }),
            );
        }
    }
    Ok(())
}

/// The SHA-256 a `Repr-Digest` header (RFC 9530) gives.
fn digest_of(header: &str) -> Option<Vec<u8>> {
    header.split(',').find_map(|member| {
        let encoded = member.trim().strip_prefix("sha-256=:")?.strip_suffix(':')?;
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .ok()
    })
}

/// Writes a backup's archive to `to`, removing what was written when the
/// bytes are not those the API vouched for.
async fn download(api: &Api, id: &str, to: PathBuf, output: &Output) -> Result<()> {
    let mut response = api
        .stream(&format!("{}/archive", path(id)), &[], LASTING)
        .await?;
    let expected = response
        .headers()
        .get("repr-digest")
        .and_then(|value| value.to_str().ok())
        .and_then(digest_of)
        .ok_or_else(|| CliError::Failed("the archive came without its digest".to_owned()))?;
    let stdout = to.as_os_str() == "-";
    let mut sink: Box<dyn Write> = if stdout {
        Box::new(std::io::stdout().lock())
    } else {
        Box::new(private_file(&to)?)
    };
    let unwritable =
        |error: std::io::Error| CliError::Failed(format!("cannot write {}: {error}", to.display()));
    let mut hasher = Sha256::new();
    let mut written = 0u64;
    let received: Result<()> = async {
        while let Some(chunk) = response.chunk().await.map_err(CliError::transport)? {
            hasher.update(&chunk);
            sink.write_all(&chunk).map_err(unwritable)?;
            written += chunk.len() as u64;
        }
        sink.flush().map_err(unwritable)
    }
    .await;
    drop(sink);
    let intact = received.is_ok() && hasher.finalize().as_slice() == expected.as_slice();
    if !intact {
        if !stdout {
            let _ = std::fs::remove_file(&to);
        }
        return Err(received.err().unwrap_or_else(|| {
            CliError::Failed("the archive is not the one the API vouched for".to_owned())
        }));
    }
    if !stdout && !output.quiet {
        eprintln!("wrote {written} bytes to {}", to.display());
    }
    Ok(())
}

/// A file only its owner reads, since an archive holds password hashes and
/// sealed keys.
pub(crate) fn private_file(path: &PathBuf) -> Result<File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options
        .open(path)
        .map_err(|error| CliError::Failed(format!("cannot write {}: {error}", path.display())))
}
