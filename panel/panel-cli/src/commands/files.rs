//! The files below the static sites' directory (ADR 0034).

use crate::{
    client::{Api, CliError, Result},
    output::{bytes, text, Column, Format, Output},
};
use clap::Subcommand;
use reqwest::{header, Method};
use std::{
    io::{Read, Write},
    path::PathBuf,
    time::Duration,
};

/// How long a file of up to 64 MiB may take to arrive.
const DOWNLOAD_LASTING: Duration = Duration::from_secs(10 * 60);

#[derive(Subcommand)]
pub(crate) enum FilesCommand {
    /// A directory below the sites' directory, the directory itself by
    /// default.
    Ls { path: Option<String> },
    /// Prints a file.
    Cat { path: String },
    /// Downloads a file, to its own name in the working directory by default.
    Get {
        path: String,
        /// Where to write it; `-` for standard output.
        #[arg(long)]
        to: Option<PathBuf>,
    },
    /// Uploads a local file, `-` for standard input, replacing what the path
    /// holds.
    Put {
        local: PathBuf,
        path: String,
        /// Only creates the file, refusing one that is there.
        #[arg(long, conflicts_with = "if_match")]
        new: bool,
        /// Replaces only the file of this entity tag.
        #[arg(long)]
        if_match: Option<String>,
    },
    /// Creates a directory and the directories above it.
    Mkdir { path: String },
    /// Removes a file, a link or an empty directory.
    Rm {
        path: String,
        /// Removes a directory with what it holds.
        #[arg(long, short)]
        recursive: bool,
        /// Confirms the removal.
        #[arg(long)]
        yes: bool,
    },
}

const ENTRIES: &[Column] = &[
    ("NAME", |entry| {
        let name = text(&entry["name"]);
        if entry["kind"] == "directory" {
            format!("{name}/")
        } else {
            name
        }
    }),
    ("KIND", |entry| text(&entry["kind"])),
    ("SIZE", |entry| {
        if entry["kind"] == "file" {
            bytes(&entry["size_bytes"])
        } else {
            "-".into()
        }
    }),
    ("MODIFIED", |entry| text(&entry["modified"])),
];

async fn download(api: &Api, path: &str) -> Result<Vec<u8>> {
    let response = api
        .stream(
            "/api/v1/site-files/content",
            &[("path", path.to_owned())],
            DOWNLOAD_LASTING,
        )
        .await?;
    Ok(response
        .bytes()
        .await
        .map_err(CliError::transport)?
        .to_vec())
}

pub async fn run(api: &Api, output: &Output, command: FilesCommand) -> Result<()> {
    match command {
        FilesCommand::Ls { path } => {
            let listed = api
                .get("/api/v1/site-files", &[("path", path.unwrap_or_default())])
                .await?
                .body;
            if output.format == Format::Json {
                output.json(&listed);
            } else if listed["entries"].as_array().is_some_and(Vec::is_empty) {
                if !output.quiet {
                    eprintln!("the directory is empty");
                }
            } else {
                output.list(&listed["entries"], ENTRIES);
            }
        }
        FilesCommand::Cat { path } => {
            let content = download(api, &path).await?;
            std::io::stdout()
                .write_all(&content)
                .map_err(|error| CliError::Failed(format!("cannot print: {error}")))?;
        }
        FilesCommand::Get { path, to } => {
            let content = download(api, &path).await?;
            let to = to.unwrap_or_else(|| {
                PathBuf::from(
                    path.trim_end_matches('/')
                        .rsplit('/')
                        .next()
                        .unwrap_or("file"),
                )
            });
            if to.as_os_str() == "-" {
                std::io::stdout()
                    .write_all(&content)
                    .map_err(|error| CliError::Failed(format!("cannot print: {error}")))?;
            } else {
                std::fs::write(&to, &content).map_err(|error| {
                    CliError::Failed(format!("cannot write {}: {error}", to.display()))
                })?;
                if !output.quiet {
                    eprintln!("wrote {} bytes to {}", content.len(), to.display());
                }
            }
        }
        FilesCommand::Put {
            local,
            path,
            new,
            if_match,
        } => {
            let content = if local.as_os_str() == "-" {
                let mut content = Vec::new();
                std::io::stdin().read_to_end(&mut content).map(|_| content)
            } else {
                std::fs::read(&local)
            }
            .map_err(|error| {
                CliError::Usage(format!("cannot read {}: {error}", local.display()))
            })?;
            let condition = match (new, if_match) {
                (true, _) => vec![(header::IF_NONE_MATCH, "*".to_owned())],
                (false, Some(tag)) => vec![(header::IF_MATCH, tag)],
                (false, None) => Vec::new(),
            };
            let written = api
                .change_bytes(
                    Method::PUT,
                    "/api/v1/site-files/content",
                    &[("path", path)],
                    Some(content),
                    &condition,
                )
                .await?;
            if output.format == Format::Json {
                output.json(&written.body);
            } else if !output.quiet {
                println!(
                    "{} {} ({}, sha256 {})",
                    if written.body["created"] == true {
                        "created"
                    } else {
                        "replaced"
                    },
                    text(&written.body["path"]),
                    bytes(&written.body["size_bytes"]),
                    text(&written.body["sha256"])
                );
            }
        }
        FilesCommand::Mkdir { path } => {
            api.change_bytes(
                Method::POST,
                "/api/v1/site-files/directories",
                &[("path", path.clone())],
                None,
                &[],
            )
            .await?;
            if !output.quiet {
                println!("created {path}");
            }
        }
        FilesCommand::Rm {
            path,
            recursive,
            yes,
        } => {
            if !yes {
                return Err(CliError::Usage(
                    "the removal is for good; pass --yes to confirm".into(),
                ));
            }
            let removed = api
                .change_bytes(
                    Method::DELETE,
                    "/api/v1/site-files",
                    &[("path", path), ("recursive", recursive.to_string())],
                    None,
                    &[],
                )
                .await?
                .body;
            if output.format == Format::Json {
                output.json(&removed);
            } else if !output.quiet {
                println!(
                    "removed {} ({} entries)",
                    text(&removed["path"]),
                    text(&removed["removed"])
                );
            }
        }
    }
    Ok(())
}
