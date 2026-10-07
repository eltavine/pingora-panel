//! The draft configuration: its files in the configuration language, checks,
//! plans and applying it.

use crate::{
    client::{Api, CliError, Result},
    commands::approvals,
    output::{text, Format, Output},
};
use clap::Subcommand;
use reqwest::{Method, StatusCode};
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeSet,
    io::{IsTerminal, Write},
    path::{Component, Path, PathBuf},
};

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
        #[arg(long, conflicts_with = "bundle")]
        dir: Option<PathBuf>,
        /// Write the whole configuration to this file as a bundle that
        /// another installation imports; `-` prints it.
        #[arg(long)]
        bundle: Option<PathBuf>,
    },
    /// Replaces the draft with configuration files or a bundle.
    Import {
        /// A bundle, `-` for a bundle on standard input, a file read as
        /// `main.conf`, or a directory of `.conf` and `.lua` files.
        path: PathBuf,
        /// Refuse if the draft changed since this version.
        #[arg(long)]
        expected_version: Option<u64>,
    },
    /// Checks configuration files without saving them; the draft by default.
    Check {
        /// A file read as `main.conf`, or a directory of `.conf` and `.lua` files.
        path: Option<PathBuf>,
    },
    /// Formats configuration files canonically.
    Fmt {
        /// A file read as `main.conf`, or a directory of `.conf` and `.lua` files.
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
        /// A file read as `main.conf`, or a directory of `.conf` and `.lua` files; the
        /// draft by default.
        path: Option<PathBuf>,
        /// The file to show.
        #[arg(long, default_value = ENTRY)]
        file: String,
    },
    /// What applies in the server, route, listener, upstream or TLS profile
    /// written at a position, and where each value comes from.
    Explain {
        /// The position, as FILE:LINE or FILE:LINE.COLUMN.
        at: String,
        /// A file read as `main.conf`, or a directory of `.conf` and `.lua` files; the
        /// draft by default.
        path: Option<PathBuf>,
    },
    /// The runtime snapshot the saved draft compiles to, as JSON.
    Ir,
    /// What applying the draft would change on the gateway, and the digest
    /// that names the plan.
    Plan,
    /// Prints the plan, asks, and applies it: compiles the draft and
    /// activates it on the gateway.
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
        /// Apply without the approvals policies ask for, in an emergency;
        /// needs approval.bypass and is recorded.
        #[arg(long, requires = "incident", conflicts_with = "dry_run")]
        bypass_reason: Option<String>,
        /// The incident a bypass answers, such as a ticket reference.
        #[arg(long, requires = "bypass_reason")]
        incident: Option<String>,
        /// Applies the plan as it is now without asking, as scripts do.
        #[arg(long, conflicts_with = "plan")]
        yes: bool,
        /// Applies only while the plan is the one with this digest, as
        /// `ppanel config plan` printed it.
        #[arg(long, value_name = "DIGEST", conflicts_with = "dry_run")]
        plan: Option<String>,
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
/// A configuration bundle at `path`, when the file is one; `-` reads one
/// from standard input.
fn bundle(path: &Path) -> Result<Option<Value>> {
    let stdin = path.as_os_str() == "-";
    if !stdin && (!path.is_file() || path.extension().is_none_or(|extension| extension != "json")) {
        return Ok(None);
    }
    let unreadable =
        |error: String| CliError::Usage(format!("cannot read {}: {error}", path.display()));
    let text = if stdin {
        std::io::read_to_string(std::io::stdin())
    } else {
        std::fs::read_to_string(path)
    }
    .map_err(|error| unreadable(error.to_string()))?;
    let bundle: Value =
        serde_json::from_str(&text).map_err(|error| unreadable(error.to_string()))?;
    Ok(Some(bundle))
}

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
                    .is_some_and(|extension| extension == "conf" || extension == "lua")
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

/// The file, 1-based line and column of `FILE:LINE` or `FILE:LINE.COLUMN`.
fn position(at: &str) -> Result<(String, usize, usize)> {
    let invalid = || CliError::Usage(format!("{at:?} is not FILE:LINE or FILE:LINE.COLUMN"));
    let (file, place) = at.rsplit_once(':').ok_or_else(invalid)?;
    let (line, column) = place.split_once('.').unwrap_or((place, "1"));
    match (line.parse(), column.parse()) {
        (Ok(line), Ok(column)) if !file.is_empty() && line > 0 && column > 0 => {
            Ok((file.to_owned(), line, column))
        }
        _ => Err(invalid()),
    }
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

pub(crate) fn rejected(detail: &str) -> CliError {
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

/// The plan's changes, then what it compares and the digest naming it.
fn print_plan(output: &Output, plan: &Value) {
    print_changes(output, plan);
    if output.quiet || output.format == Format::Json {
        return;
    }
    let against = match plan["active_revision"].as_u64() {
        Some(revision) => format!("revision {revision}"),
        None => "nothing applied yet".to_owned(),
    };
    println!();
    println!(
        "Plan {} for version {} against {against}",
        text(&plan["digest"]),
        text(&plan["draft_version"])
    );
}

/// Asks on the terminal whether to apply the plan printed above.
fn confirm_apply(output: &Output) -> Result<()> {
    let terminal = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    if !terminal || output.quiet || output.format == Format::Json {
        return Err(CliError::Usage(
            "applying asks first: pass --yes to apply the plan as it is now, or --plan with \
             a digest `ppanel config plan` printed"
                .into(),
        ));
    }
    eprint!("Apply this plan to the gateway? [y/N] ");
    let _ = std::io::stderr().flush();
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(|error| CliError::Transport(format!("cannot read the answer: {error}")))?;
    if matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        Err(CliError::Failed("nothing was applied".into()))
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

pub(crate) async fn draft_files(api: &Api, path: Option<PathBuf>) -> Result<Map<String, Value>> {
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

/// Applies the draft; the reply is the applied revision, or with 202 the
/// approval request the change waits on.
async fn apply(api: &Api, body: Value) -> Result<(Value, bool)> {
    let reply = api
        .change(Method::POST, "/api/v1/config/apply", Some(&body), None)
        .await?;
    Ok((reply.body, reply.status == StatusCode::ACCEPTED))
}

fn applied_message((applied, waiting): &(Value, bool)) -> String {
    if *waiting {
        return approvals::waiting_message(applied);
    }
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
        ConfigCommand::Export {
            bundle: Some(bundle),
            ..
        } => {
            let exported = api.get("/api/v1/config/bundle", &[]).await?.body;
            let text = format!(
                "{}\n",
                serde_json::to_string_pretty(&exported).expect("JSON values serialize")
            );
            if bundle.as_os_str() == "-" {
                if !output.quiet {
                    print!("{text}");
                }
            } else {
                std::fs::write(&bundle, text).map_err(|error| {
                    CliError::Usage(format!("cannot write {}: {error}", bundle.display()))
                })?;
                output.done(
                    &format!(
                        "Wrote {} files to {} as a bundle",
                        exported["files"].as_object().map_or(0, Map::len),
                        bundle.display()
                    ),
                    &exported,
                );
            }
        }
        ConfigCommand::Export { dir, .. } => {
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
            let if_match = expected_version.map(|version| format!("\"draft-{version}\""));
            let (route, body) = match bundle(&path)? {
                Some(bundle) => ("/api/v1/config/bundle", bundle),
                None => (
                    "/api/v1/config/source",
                    json!({ "files": read_files(&path)? }),
                ),
            };
            let files = body["files"].as_object().map_or(0, Map::len);
            let saved = api
                .change(Method::PUT, route, Some(&body), if_match.as_deref())
                .await?;
            print_diagnostics(&saved.body["diagnostics"]);
            output.done(
                &format!(
                    "Saved {} files as draft {}",
                    files,
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
        ConfigCommand::Explain { at, path } => {
            let (file, line, column) = position(&at)?;
            let files = draft_files(api, path).await?;
            let explained = api
                .post_read(
                    "/api/v1/config/explain",
                    &json!({ "files": files, "file": file, "line": line, "column": column }),
                )
                .await?
                .body;
            if output.format == Format::Json {
                output.json(&explained);
                return Ok(());
            }
            if output.quiet {
                return Ok(());
            }
            let name = explained["name"]
                .as_str()
                .map(|name| format!(" {name}"))
                .unwrap_or_default();
            println!(
                "{}{name} at {}",
                text(&explained["block"]),
                text(&explained["source_span"])
            );
            output.list(
                &explained["settings"],
                &[
                    ("SETTING", |setting| text(&setting["name"])),
                    ("FOR", |setting| text(&setting["scope"])),
                    ("VALUE", |setting| match text(&setting["value"]) {
                        value if value.is_empty() => "-".into(),
                        value => value,
                    }),
                    ("FROM", |setting| match setting["source"].as_str() {
                        Some("inherited") => text(&setting["from"]),
                        Some(source) => source.into(),
                        None => "-".into(),
                    }),
                    ("AT", |setting| text(&setting["source_span"])),
                ],
            );
            let mut explained_rules = BTreeSet::new();
            for setting in explained["settings"].as_array().into_iter().flatten() {
                if let (Some(name), Some(rule), false) = (
                    setting["name"].as_str(),
                    setting["rule"].as_str(),
                    setting["source"] == "here",
                ) {
                    if explained_rules.is_empty() {
                        println!();
                    }
                    if explained_rules.insert(name) {
                        println!("{name}: {rule}");
                    }
                }
            }
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
            print_plan(output, &plan);
        }
        ConfigCommand::Apply {
            expected_version,
            note,
            dry_run,
            bypass_reason,
            incident,
            yes,
            plan,
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
                let expected_plan = match plan {
                    Some(digest) => digest,
                    None => {
                        let reviewed = api.get("/api/v1/config/plan", &[]).await?.body;
                        if output.format != Format::Json {
                            print_plan(output, &reviewed);
                        }
                        if !yes {
                            confirm_apply(output)?;
                        }
                        text(&reviewed["digest"])
                    }
                };
                let mut body = json!({
                    "expected_version": expected_version,
                    "note": note,
                    "expected_plan": expected_plan,
                });
                if let (Some(reason), Some(incident)) = (bypass_reason, incident) {
                    body["bypass"] = json!({ "reason": reason, "incident": incident });
                }
                let applied = apply(api, body).await?;
                output.done(&applied_message(&applied), &applied.0);
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
            let applied = apply(api, json!({ "expected_version": version, "note": note })).await?;
            output.done(
                &format!(
                    "Rolled back to revision {to}. {}",
                    applied_message(&applied)
                ),
                &applied.0,
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

    #[test]
    fn positions_are_written_as_in_diagnostics() {
        assert_eq!(
            position("sites/shop.conf:12.9").unwrap(),
            ("sites/shop.conf".to_owned(), 12, 9)
        );
        assert_eq!(
            position("main.conf:3").unwrap(),
            ("main.conf".to_owned(), 3, 1)
        );
        for invalid in [
            "main.conf",
            ":3",
            "main.conf:0",
            "main.conf:3.0",
            "main.conf:x",
        ] {
            assert!(position(invalid).is_err(), "{invalid}");
        }
    }
}
