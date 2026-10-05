//! The Compose projects on the container engines the host agent reaches.

use super::time;
use crate::{
    client::{Api, CliError, Result},
    output::{text, Column, Format, Output},
};
use clap::{Args, Subcommand};
use reqwest::Method;
use serde_json::Value;
use std::io::{ErrorKind, Write};

#[derive(Args)]
pub(crate) struct Project {
    /// The project's name.
    project: String,
    /// `docker` or `podman`.
    #[arg(long, default_value = "docker")]
    engine: String,
}

impl Project {
    fn path(&self, rest: &str) -> String {
        format!(
            "/api/v1/container-engines/{}/compose-projects/{}/{rest}",
            self.engine, self.project
        )
    }
}

#[derive(Subcommand)]
pub(crate) enum ComposeCommand {
    /// An engine's Compose projects, with their services and how many of
    /// their containers run; `*` marks the panel's own installation.
    List {
        /// `docker` or `podman`.
        #[arg(long, default_value = "docker")]
        engine: String,
    },
    /// Starts a project's containers that are not running.
    Up {
        #[command(flatten)]
        target: Project,
    },
    /// Stops and removes a project's containers and networks, and keeps its
    /// volumes; only Compose brings it back.
    Down {
        #[command(flatten)]
        target: Project,
        #[arg(long)]
        yes: bool,
    },
    /// Restarts a project's containers.
    Restart {
        #[command(flatten)]
        target: Project,
        #[arg(long)]
        yes: bool,
    },
    /// The last lines a project's containers printed, merged by time, each
    /// after its container's name. Standard error goes to standard error.
    Logs {
        #[command(flatten)]
        target: Project,
        /// How many of their last lines to show together, at most 5000.
        #[arg(long, short = 'n', default_value_t = 200,
              value_parser = clap::value_parser!(u32).range(1..=5000))]
        lines: u32,
        /// Only lines printed since this, in RFC 3339 or as how long ago such
        /// as `1h`.
        #[arg(long, value_parser = time)]
        since: Option<String>,
        /// Puts the time its engine recorded each line before it.
        #[arg(long, short)]
        timestamps: bool,
    },
    /// The Compose files a project's labels name, as they are on the host.
    Config {
        #[command(flatten)]
        target: Project,
    },
}

/// Services as `web 2/2, worker 0/1`.
fn services(project: &Value) -> String {
    let services: Vec<String> = project["services"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|service| {
            format!(
                "{} {}/{}",
                text(&service["name"]),
                text(&service["running"]),
                text(&service["containers"])
            )
        })
        .collect();
    if services.is_empty() {
        "-".into()
    } else {
        services.join(", ")
    }
}

const PROJECTS: &[Column] = &[
    ("NAME", |project| text(&project["name"])),
    ("PANEL", |project| {
        if project["installation"] == true {
            "*".into()
        } else {
            String::new()
        }
    }),
    ("RUNNING", |project| {
        format!(
            "{}/{}",
            text(&project["running"]),
            text(&project["containers"])
        )
    }),
    ("SERVICES", services),
    ("DIRECTORY", |project| text(&project["working_directory"])),
];

pub async fn run(api: &Api, output: &Output, command: ComposeCommand) -> Result<()> {
    match command {
        ComposeCommand::List { engine } => {
            let path = format!("/api/v1/container-engines/{engine}/compose-projects");
            let list = api.get(&path, &[]).await?.body;
            if output.format == Format::Json {
                output.json(&list);
            } else if list["projects"].as_array().is_some_and(Vec::is_empty) {
                if !output.quiet {
                    eprintln!("the engine has no Compose projects");
                }
            } else {
                output.list(&list["projects"], PROJECTS);
            }
        }
        ComposeCommand::Up { target } => act(api, output, &target, "up").await?,
        ComposeCommand::Down { target, yes } => {
            confirmed(yes)?;
            act(api, output, &target, "down").await?;
        }
        ComposeCommand::Restart { target, yes } => {
            confirmed(yes)?;
            act(api, output, &target, "restart").await?;
        }
        ComposeCommand::Logs {
            target,
            lines,
            since,
            timestamps,
        } => {
            let mut query = vec![("lines", lines.to_string())];
            query.extend(since.map(|since| ("since", since)));
            let logs = api.get(&target.path("logs"), &query).await?.body;
            if output.format == Format::Json {
                output.json(&logs);
            } else {
                print(output, &logs["lines"], timestamps)?;
                if logs["truncated"] == true && !output.quiet {
                    eprintln!("older lines were left out; ask for fewer or use --since");
                }
            }
        }
        ComposeCommand::Config { target } => {
            let files = api.get(&target.path("files"), &[]).await?.body;
            if output.format == Format::Json {
                output.json(&files);
            } else {
                config(output, &files["files"])?;
            }
        }
    }
    Ok(())
}

fn confirmed(yes: bool) -> Result<()> {
    if yes {
        Ok(())
    } else {
        Err(CliError::Usage(
            "what runs in the project stops; pass --yes to confirm".into(),
        ))
    }
}

async fn act(api: &Api, output: &Output, target: &Project, action: &str) -> Result<()> {
    let change = api
        .change(Method::POST, &target.path(action), None, None)
        .await?
        .body;
    if output.format == Format::Json {
        output.json(&change);
    } else if !output.quiet {
        match &change["project"] {
            Value::Null => println!(
                "{} is down; {} containers removed",
                target.project,
                text(&change["changed"])
            ),
            project => output.item(project, PROJECTS),
        }
    }
    let failures = change["failures"].as_array().map_or(0, Vec::len);
    for failure in change["failures"].as_array().into_iter().flatten() {
        eprintln!(
            "{}: {}",
            text(&failure["name"]),
            text(&failure["error"]["message"])
        );
    }
    if failures == 0 {
        Ok(())
    } else {
        Err(CliError::Failed(format!(
            "the engine refused {failures} of the project's containers or networks"
        )))
    }
}

/// Prints lines after their containers' names, as `docker compose logs`
/// does, or as JSON lines, until the output is closed.
fn print(output: &Output, lines: &Value, timestamps: bool) -> Result<()> {
    if output.quiet {
        return Ok(());
    }
    let lines = lines.as_array().map(Vec::as_slice).unwrap_or_default();
    let width = lines
        .iter()
        .filter_map(|line| line["container"].as_str())
        .map(|name| name.chars().count())
        .max()
        .unwrap_or_default();
    let (mut stdout, mut stderr) = (std::io::stdout().lock(), std::io::stderr().lock());
    for line in lines {
        let printed = &line["line"];
        let sink: &mut dyn Write =
            if printed["stream"] == "stderr" && output.format == Format::Table {
                &mut stderr
            } else {
                &mut stdout
            };
        let container = line["container"].as_str().unwrap_or_default();
        let text = printed["text"].as_str().unwrap_or_default();
        let written = match (output.format, timestamps) {
            (Format::Json, _) => writeln!(sink, "{line}"),
            (Format::Table, true) => writeln!(
                sink,
                "{container:width$} | {} {text}",
                printed["time"].as_str().unwrap_or_default()
            ),
            (Format::Table, false) => writeln!(sink, "{container:width$} | {text}"),
        };
        match written {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::BrokenPipe => return Ok(()),
            Err(error) => return Err(CliError::Failed(format!("cannot print: {error}"))),
        }
    }
    match stdout.flush().and_then(|()| stderr.flush()) {
        Err(error) if error.kind() != ErrorKind::BrokenPipe => {
            Err(CliError::Failed(format!("cannot print: {error}")))
        }
        _ => Ok(()),
    }
}

/// Prints each file after a comment naming it, and says on standard error
/// which could not be read.
fn config(output: &Output, files: &Value) -> Result<()> {
    if output.quiet {
        return Ok(());
    }
    let mut unread = 0;
    for (index, file) in files.as_array().into_iter().flatten().enumerate() {
        match file["content"].as_str() {
            Some(content) => {
                if index > 0 {
                    println!();
                }
                println!("# {}", text(&file["path"]));
                print!("{content}");
                if !content.ends_with('\n') {
                    println!();
                }
            }
            None => {
                unread += 1;
                eprintln!(
                    "cannot read {}: {}",
                    text(&file["path"]),
                    text(&file["error"]["message"])
                );
            }
        }
    }
    if unread == 0 {
        Ok(())
    } else {
        Err(CliError::Failed(format!(
            "{unread} of the project's Compose files could not be read"
        )))
    }
}
