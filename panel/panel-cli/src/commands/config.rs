//! The draft configuration: its files in the configuration language, checks,
//! plans and applying it.

use crate::{
    client::{Api, CliError, Result},
    output::{text, Format, Output},
};
use clap::Subcommand;
use reqwest::{Method, StatusCode};
use serde_json::{json, Map, Value};
use std::path::{Component, Path, PathBuf};

/// The file every configuration starts from.
const ENTRY: &str = "main.conf";

#[derive(Subcommand)]
pub(crate) enum ConfigCommand {
    /// The draft's version and whether the gateway runs it.
    Draft,
    /// Validates the draft, or only the given sites.
    Validate {
        #[arg(long = "site")]
        sites: Vec<String>,
    },
    /// Writes the draft's files to a directory, or prints `main.conf`.
    Export {
        /// Directory to write the files to; created when missing.
        #[arg(long)]
        dir: Option<PathBuf>,
    },
    /// Replaces the draft with configuration files.
    Import {
        /// A file read as `main.conf`, or a directory of `.conf` files.
        path: PathBuf,
        /// Refuse if the draft changed since this version.
        #[arg(long)]
        expected_version: Option<u64>,
    },
    /// Checks configuration files without saving them; the draft by default.
    Check {
        /// A file read as `main.conf`, or a directory of `.conf` files.
        path: Option<PathBuf>,
    },
    /// Formats configuration files canonically.
    Fmt {
        /// A file read as `main.conf`, or a directory of `.conf` files.
        path: PathBuf,
        /// List files that are not formatted and fail instead of printing.
        #[arg(long, conflicts_with = "write")]
        check: bool,
        /// Rewrite files that are not formatted.
        #[arg(long)]
        write: bool,
    },
    /// Converts NGINX configuration to the language, reporting every
    /// directive that does not carry over.
    ImportNginx {
        /// The main NGINX file, read with the files of its directory, or a
        /// directory holding `--entry`.
        path: PathBuf,
        /// The main file within a directory.
        #[arg(long, default_value = "nginx.conf")]
        entry: String,
        /// Write the converted files to this directory.
        #[arg(long, conflicts_with = "save")]
        dir: Option<PathBuf>,
        /// Replace the draft with the converted files.
        #[arg(long)]
        save: bool,
        /// With --save, refuse if the draft changed since this version.
        #[arg(long, requires = "save")]
        expected_version: Option<u64>,
    },
    /// The directives of the configuration language.
    Schema,
    /// The syntax tree of a file: its directives as the language reads them.
    Ast {
        /// A file read as `main.conf`, or a directory of `.conf` files; the
        /// draft by default.
        path: Option<PathBuf>,
        /// The file to show.
        #[arg(long, default_value = ENTRY)]
        file: String,
    },
    /// The runtime snapshot the saved draft compiles to, as JSON.
    Ir,
    /// What applying the draft would change on the gateway.
    Plan,
    /// Compiles the draft and activates it on the gateway.
    Apply {
        /// Refuse if the draft changed since this version.
        #[arg(long)]
        expected_version: Option<u64>,
        /// Recorded with the revision, for example why it is applied.
        #[arg(long)]
        note: Option<String>,
        /// Run every check, the gateway's preparation included, without
        /// activating anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Restores a revision into the draft and applies it.
    Rollback {
        #[arg(long, value_name = "REVISION")]
        to: u64,
        /// Why, recorded with the new revision.
        #[arg(long)]
        reason: Option<String>,
    },
}

/// Configuration files by their path relative to `path`, which is either the
/// entry file itself or a directory holding it.
pub(crate) fn read_files(path: &Path) -> Result<Map<String, Value>> {
    let unreadable =
        |error: std::io::Error| CliError::Usage(format!("cannot read {}: {error}", path.display()));
    let mut files = Map::new();
    if path.is_dir() {
        let mut pending = vec![path.to_path_buf()];
        while let Some(directory) = pending.pop() {
            for entry in std::fs::read_dir(&directory).map_err(unreadable)? {
                let entry = entry.map_err(unreadable)?.path();
                if entry
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with('.'))
                {
                    continue;
                }
                if entry.is_dir() {
                    pending.push(entry);
                } else if entry
                    .extension()
                    .is_some_and(|extension| extension == "conf")
                {
                    let relative = entry
                        .strip_prefix(path)
                        .expect("entries are below the directory")
                        .components()
                        .map(|component| component.as_os_str().to_string_lossy())
                        .collect::<Vec<_>>()
                        .join("/");
                    let text = std::fs::read_to_string(&entry).map_err(unreadable)?;
                    files.insert(relative, json!(text));
                }
            }
        }
        if !files.contains_key(ENTRY) {
            return Err(CliError::Usage(format!(
                "{} has no {ENTRY}",
                path.display()
            )));
        }
    } else {
        let text = std::fs::read_to_string(path).map_err(unreadable)?;
        files.insert(ENTRY.into(), json!(text));
    }
    Ok(files)
}

/// Every UTF-8 file below `directory` by its relative path, for NGINX
/// configuration whose includes may name any file.
fn read_tree(directory: &Path) -> Result<Map<String, Value>> {
    let unreadable = |error: std::io::Error| {
        CliError::Usage(format!("cannot read {}: {error}", directory.display()))
    };
    let mut files = Map::new();
    let mut pending = vec![directory.to_path_buf()];
    while let Some(current) = pending.pop() {
        for entry in std::fs::read_dir(&current).map_err(unreadable)? {
            let path = entry.map_err(unreadable)?.path();
            if path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with('.'))
            {
                continue;
            }
            if path.is_dir() {
                pending.push(path);
            } else if let Ok(text) = std::fs::read_to_string(&path) {
                let relative = path
                    .strip_prefix(directory)
                    .expect("entries are below the directory")
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                files.insert(relative, json!(text));
            }
        }
    }
    Ok(files)
}

/// Where a file named by the API goes below `directory`; names leaving the
/// directory are refused.
fn destination(directory: &Path, name: &str) -> Result<PathBuf> {
    let relative = Path::new(name);
    if name.is_empty()
        || !relative
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(CliError::Usage(format!("refusing to write file {name:?}")));
    }
    Ok(directory.join(relative))
}

fn write_files(directory: &Path, files: &Map<String, Value>) -> Result<()> {
    for (name, text) in files {
        let path = destination(directory, name)?;
        let unwritable = |error: std::io::Error| {
            CliError::Usage(format!("cannot write {}: {error}", path.display()))
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(unwritable)?;
        }
        std::fs::write(&path, text.as_str().unwrap_or_default()).map_err(unwritable)?;
    }
    Ok(())
}

/// Diagnostics as compilers print them, `file:line.column: severity: message`.
pub(crate) fn print_diagnostics(diagnostics: &Value) {
    for diagnostic in diagnostics.as_array().into_iter().flatten() {
        let place = diagnostic["source_span"]
            .as_str()
            .or_else(|| diagnostic["resource_id"].as_str())
            .unwrap_or("configuration");
        let severity = diagnostic["severity"]
            .as_str()
            .unwrap_or("error")
            .to_lowercase();
        eprintln!(
            "{place}: {severity}: {} [{}]",
            text(&diagnostic["message"]),
            text(&diagnostic["code"])
        );
    }
}

fn rejected(detail: &str) -> CliError {
    CliError::Api {
        status: StatusCode::UNPROCESSABLE_ENTITY,
        problem: json!({ "detail": detail }),
    }
}

/// Unified differences of a plan or revision comparison.
pub(crate) fn print_changes(output: &Output, changes: &Value) {
    if output.quiet {
        return;
    }
    if output.format == Format::Json {
        return output.json(changes);
    }
    let resources = changes["resources"].as_array().cloned().unwrap_or_default();
    if resources.is_empty() {
        println!("No changes.");
        return;
    }
    output.list(
        &json!(resources),
        &[
            ("CHANGE", |change| text(&change["change"])),
            ("RESOURCE", |change| text(&change["resource"])),
        ],
    );
    for file in changes["files"].as_array().into_iter().flatten() {
        println!();
        print!("{}", text(&file["diff"]));
    }
}

/// Directives as an indented outline with their places.
fn print_tree(nodes: &Value, depth: usize) {
    let indent = "    ".repeat(depth);
    for node in nodes.as_array().into_iter().flatten() {
        for comment in node["comments"].as_array().into_iter().flatten() {
            println!("{indent}# {}", text(comment));
        }
        let mut line = text(&node["name"]);
        for arg in node["args"].as_array().into_iter().flatten() {
            line.push(' ');
            line.push_str(&text(arg));
        }
        println!("{indent}{line}  [{}]", text(&node["span"]));
        if node.get("block").is_some() {
            print_tree(&node["block"], depth + 1);
        }
    }
}

async fn draft_files(api: &Api, path: Option<PathBuf>) -> Result<Map<String, Value>> {
    match path {
        Some(path) => read_files(&path),
        None => Ok(api.get("/api/v1/config/source", &[]).await?.body["files"]
            .as_object()
            .cloned()
            .unwrap_or_default()),
    }
}

async fn draft_version(api: &Api) -> Result<u64> {
    let draft = api.get("/api/v1/config/draft", &[]).await?.body;
    draft["version"]
        .as_u64()
        .ok_or_else(|| CliError::Transport("the draft has no version".into()))
}

async fn apply(api: &Api, expected_version: Option<u64>, note: Option<String>) -> Result<Value> {
    Ok(api
        .change(
            Method::POST,
            "/api/v1/config/apply",
            Some(&json!({ "expected_version": expected_version, "note": note })),
            None,
        )
        .await?
        .body)
}

fn applied_message(applied: &Value) -> String {
    format!(
        "Applied version {} as revision {} (gateway revision {}, {})",
        text(&applied["draft"]["version"]),
        text(&applied["revision"]),
        text(&applied["revision_id"]),
        text(&applied["content_hash"])
    )
}

pub async fn run(api: &Api, output: &Output, command: ConfigCommand) -> Result<()> {
    match command {
        ConfigCommand::Draft => {
            let draft = api.get("/api/v1/config/draft", &[]).await?.body;
            output.item(
                &draft,
                &[
                    ("Version", |draft| text(&draft["version"])),
                    ("Updated", |draft| text(&draft["updated_at"])),
                    ("Applied version", |draft| text(&draft["applied_version"])),
                    ("Applied", |draft| text(&draft["applied_at"])),
                    ("Pending changes", |draft| text(&draft["pending"])),
                ],
            );
        }
        ConfigCommand::Validate { sites } => {
            let query: Vec<(&str, String)> = if sites.is_empty() {
                Vec::new()
            } else {
                vec![("site_ids", sites.join(","))]
            };
            let result = api.get("/api/v1/config/validation", &query).await?.body;
            output.list(
                &result["diagnostics"],
                &[
                    ("RESOURCE", |diagnostic| text(&diagnostic["resource_id"])),
                    ("MESSAGE", |diagnostic| text(&diagnostic["message"])),
                ],
            );
            if result["valid"] != true {
                return Err(rejected("the configuration has errors"));
            }
            output.done("The configuration is valid", &result);
        }
        ConfigCommand::Export { dir } => {
            let source = api.get("/api/v1/config/source", &[]).await?.body;
            let files = source["files"].as_object().cloned().unwrap_or_default();
            match dir {
                Some(dir) => {
                    write_files(&dir, &files)?;
                    output.done(
                        &format!("Wrote {} files to {}", files.len(), dir.display()),
                        &source,
                    );
                }
                None if output.format == Format::Json => output.json(&source),
                None if files.len() == 1 => {
                    if !output.quiet {
                        print!("{}", text(&files[ENTRY]));
                    }
                }
                None => {
                    return Err(CliError::Usage(format!(
                        "the draft has {} files; pass --dir to write them",
                        files.len()
                    )))
                }
            }
        }
        ConfigCommand::Import {
            path,
            expected_version,
        } => {
            let files = read_files(&path)?;
            let if_match = expected_version.map(|version| format!("\"draft-{version}\""));
            let saved = api
                .change(
                    Method::PUT,
                    "/api/v1/config/source",
                    Some(&json!({ "files": files })),
                    if_match.as_deref(),
                )
                .await?;
            print_diagnostics(&saved.body["diagnostics"]);
            output.done(
                &format!(
                    "Saved {} files as draft {}",
                    files.len(),
                    saved
                        .etag
                        .as_deref()
                        .unwrap_or_default()
                        .trim_matches('"')
                        .trim_start_matches("draft-")
                ),
                &saved.body,
            );
        }
        ConfigCommand::Check { path } => {
            let files = draft_files(api, path).await?;
            let result = api
                .post_read("/api/v1/config/check", &json!({ "files": files }))
                .await?
                .body;
            if output.format == Format::Json {
                output.json(&result);
            } else {
                print_diagnostics(&result["diagnostics"]);
            }
            if result["valid"] != true {
                return Err(rejected("the configuration has errors"));
            }
            if output.format != Format::Json {
                output.done("The configuration is valid", &result);
            }
        }
        ConfigCommand::Fmt { path, check, write } => {
            let files = read_files(&path)?;
            let result = api
                .post_read("/api/v1/config/format", &json!({ "files": files }))
                .await?
                .body;
            print_diagnostics(&result["diagnostics"]);
            let formatted = result["files"].as_object().cloned().unwrap_or_default();
            let changed: Map<String, Value> = formatted
                .into_iter()
                .filter(|(name, text)| files.get(name) != Some(text))
                .collect();
            if check {
                for name in changed.keys() {
                    if !output.quiet {
                        println!("{name}");
                    }
                }
                if !changed.is_empty() {
                    return Err(rejected("some files are not formatted"));
                }
            } else if write {
                for (name, text) in &changed {
                    let target = if path.is_dir() {
                        destination(&path, name)?
                    } else {
                        path.clone()
                    };
                    std::fs::write(&target, text.as_str().unwrap_or_default()).map_err(
                        |error| {
                            CliError::Usage(format!("cannot write {}: {error}", target.display()))
                        },
                    )?;
                }
                output.done(&format!("Formatted {} files", changed.len()), &result);
            } else if output.format == Format::Json {
                output.json(&result);
            } else if !output.quiet {
                for (name, text) in result["files"].as_object().into_iter().flatten() {
                    if files.len() > 1 {
                        println!("# {name}");
                    }
                    print!("{}", text.as_str().unwrap_or_default());
                }
            }
            if result["diagnostics"]
                .as_array()
                .is_some_and(|diagnostics| !diagnostics.is_empty())
            {
                return Err(rejected("some files have syntax errors"));
            }
        }
        ConfigCommand::ImportNginx {
            path,
            entry,
            dir,
            save,
            expected_version,
        } => {
            let (directory, entry) = if path.is_dir() {
                (path.clone(), entry)
            } else {
                let name = path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                (
                    path.parent()
                        .filter(|parent| !parent.as_os_str().is_empty())
                        .map_or_else(|| PathBuf::from("."), Path::to_path_buf),
                    name,
                )
            };
            let files = read_tree(&directory)?;
            if !files.contains_key(&entry) {
                return Err(CliError::Usage(format!(
                    "{} has no {entry}",
                    directory.display()
                )));
            }
            let imported = api
                .post_read(
                    "/api/v1/config/import/nginx",
                    &json!({ "files": files, "entry": entry }),
                )
                .await?
                .body;
            print_diagnostics(&imported["report"]);
            if imported["valid"] != true {
                print_diagnostics(&imported["diagnostics"]);
                return Err(rejected("the converted configuration has errors"));
            }
            let converted = imported["files"].as_object().cloned().unwrap_or_default();
            if let Some(dir) = dir {
                write_files(&dir, &converted)?;
                output.done(
                    &format!("Wrote {} files to {}", converted.len(), dir.display()),
                    &imported,
                );
            } else if save {
                let if_match = expected_version.map(|version| format!("\"draft-{version}\""));
                let saved = api
                    .change(
                        Method::PUT,
                        "/api/v1/config/source",
                        Some(&json!({ "files": converted })),
                        if_match.as_deref(),
                    )
                    .await?
                    .body;
                output.done(
                    &format!(
                        "Saved the converted configuration as draft {}",
                        text(&saved["version"])
                    ),
                    &saved,
                );
            } else if output.format == Format::Json {
                output.json(&imported);
            } else if !output.quiet {
                print!("{}", text(&converted[ENTRY]));
            }
        }
        ConfigCommand::Schema => {
            let schema = api.get("/api/v1/config/schema", &[]).await?.body;
            if output.format == Format::Json {
                output.json(&schema);
                return Ok(());
            }
            output.list(
                &schema["directives"],
                &[
                    ("DIRECTIVE", |directive| text(&directive["name"])),
                    ("CONTEXTS", |directive| text(&directive["contexts"])),
                    ("SYNTAX", |directive| text(&directive["syntax"])),
                    ("DEPRECATED", |directive| {
                        text(&directive["deprecated"]["replacement"])
                    }),
                ],
            );
        }
        ConfigCommand::Ast { path, file } => {
            let files = draft_files(api, path).await?;
            let tree = api
                .post_read(
                    "/api/v1/config/ast",
                    &json!({ "files": files, "file": file }),
                )
                .await?
                .body;
            if output.format == Format::Json {
                output.json(&tree);
            } else if !output.quiet {
                print_tree(&tree["directives"], 0);
            }
            print_diagnostics(&tree["diagnostics"]);
        }
        ConfigCommand::Ir => {
            let snapshot = api.get("/api/v1/config/ir", &[]).await?.body;
            if !output.quiet {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&snapshot).expect("JSON values serialize")
                );
            }
        }
        ConfigCommand::Plan => {
            let plan = api.get("/api/v1/config/plan", &[]).await?.body;
            print_changes(output, &plan);
        }
        ConfigCommand::Apply {
            expected_version,
            note,
            dry_run,
        } => {
            if dry_run {
                let checked = api
                    .change(
                        Method::POST,
                        "/api/v1/config/dry-run",
                        Some(&json!({ "expected_version": expected_version })),
                        None,
                    )
                    .await?
                    .body;
                print_diagnostics(&checked["diagnostics"]);
                output.done(
                    &format!(
                        "Version {} passed every check; nothing was activated",
                        text(&checked["draft"]["version"])
                    ),
                    &checked,
                );
            } else {
                let applied = apply(api, expected_version, note).await?;
                output.done(&applied_message(&applied), &applied);
            }
        }
        ConfigCommand::Rollback { to, reason } => {
            api.change(
                Method::POST,
                &format!("/api/v1/revisions/{to}/restore"),
                None,
                None,
            )
            .await?;
            let version = draft_version(api).await?;
            let note = reason.unwrap_or_else(|| format!("Rollback to revision {to}"));
            let applied = apply(api, Some(version), Some(note)).await?;
            output.done(
                &format!(
                    "Rolled back to revision {to}. {}",
                    applied_message(&applied)
                ),
                &applied,
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_written_from_the_api_stay_below_the_directory() {
        let directory = Path::new("out");
        assert_eq!(
            destination(directory, "sites/shop.conf").unwrap(),
            directory.join("sites").join("shop.conf")
        );
        for name in ["", "../escape.conf", "/etc/passwd", "a/../../b.conf"] {
            assert!(destination(directory, name).is_err(), "{name}");
        }
    }

    #[test]
    fn a_directory_is_read_with_relative_paths() {
        let directory = std::env::temp_dir().join(format!("ppanel-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(directory.join("sites")).unwrap();
        std::fs::write(directory.join("main.conf"), "language_version 1;\n").unwrap();
        std::fs::write(directory.join("sites/shop.conf"), "server shop {}\n").unwrap();
        std::fs::write(directory.join("README.md"), "ignored").unwrap();
        let files = read_files(&directory).unwrap();
        std::fs::remove_dir_all(&directory).unwrap();
        let mut names: Vec<_> = files.keys().cloned().collect();
        names.sort();
        assert_eq!(names, ["main.conf", "sites/shop.conf"]);
    }
}
