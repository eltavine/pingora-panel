//! The container engines the host agent reaches and what runs on them.

use super::time;
use crate::{
    client::{Api, CliError, Result},
    output::{bytes, text, Column, Format, Output},
};
use clap::Subcommand;
use futures_util::StreamExt;
use reqwest::Method;
use serde_json::Value;
use std::io::{ErrorKind, Write};
use tokio_tungstenite::tungstenite::Message;

/// The most earlier lines sent before following.
const MOST_BACKLOG: u32 = 1_000;

#[derive(Subcommand)]
pub(crate) enum ContainerCommand {
    /// The Docker and Podman engines the host agent reaches.
    Engines,
    /// Enables or disables an engine.
    Engine {
        #[command(subcommand)]
        action: EngineAction,
    },
    /// An engine's containers.
    List {
        /// `docker` or `podman`.
        #[arg(long, default_value = "docker")]
        engine: String,
        /// Matched against names and images, ignoring case.
        #[arg(long)]
        search: Option<String>,
        /// Only containers in this state, such as `running` or `exited`;
        /// repeat for more.
        #[arg(long = "state", value_name = "STATE")]
        states: Vec<String>,
    },
    /// A container's configuration and state, with its labels, mounts and
    /// networks; never its environment or command line.
    Inspect {
        #[command(flatten)]
        target: Target,
    },
    /// What a container printed: its last lines, or, with `--follow`, the
    /// lines it prints as it prints them. Its standard error goes to
    /// standard error.
    Logs {
        #[command(flatten)]
        target: Target,
        /// How many of its last lines to show, at most 5000; at most 1000
        /// before following.
        #[arg(long, short = 'n', default_value_t = 200,
              value_parser = clap::value_parser!(u32).range(1..=5000))]
        lines: u32,
        /// Only lines printed since this, in RFC 3339 or as how long ago such
        /// as `1h`.
        #[arg(long, value_parser = time)]
        since: Option<String>,
        /// Keeps printing lines as the container prints them, until it stops
        /// or you interrupt.
        #[arg(long, short)]
        follow: bool,
        /// Starts each line with the time its engine recorded it.
        #[arg(long, short)]
        timestamps: bool,
    },
    /// What running containers use, read once as `docker stats
    /// --no-stream` reads it: CPU as a share of one CPU, memory without
    /// the page cache, network and block I/O, and processes.
    Stats {
        /// One container's ID, a unique prefix of its ID or its name; every
        /// running container when absent.
        #[arg(value_parser = reference)]
        container: Option<String>,
        /// `docker` or `podman`.
        #[arg(long, default_value = "docker")]
        engine: String,
    },
    /// Starts a container.
    Start {
        #[command(flatten)]
        target: Target,
    },
    /// Stops a container: its stop signal, then SIGKILL once its stop
    /// timeout passes.
    Stop {
        #[command(flatten)]
        target: Target,
        /// Confirms that what runs in it stops.
        #[arg(long)]
        yes: bool,
    },
    /// Restarts a container.
    Restart {
        #[command(flatten)]
        target: Target,
        /// Confirms that what runs in it stops while it restarts.
        #[arg(long)]
        yes: bool,
    },
    /// Kills a container at once with SIGKILL.
    Kill {
        #[command(flatten)]
        target: Target,
        /// Confirms that what runs in it stops without a chance to finish.
        #[arg(long)]
        yes: bool,
    },
    /// Removes a container.
    Remove {
        #[command(flatten)]
        target: Target,
        /// Kills and removes a running container instead of refusing to.
        #[arg(long)]
        force: bool,
        /// Removes its anonymous volumes with it.
        #[arg(long)]
        volumes: bool,
        /// Confirms that the container is removed.
        #[arg(long)]
        yes: bool,
    },
}

/// The container an action is taken on.
#[derive(clap::Args)]
pub(crate) struct Target {
    /// Its ID, a unique prefix of its ID or its name.
    #[arg(value_parser = reference)]
    container: String,
    /// `docker` or `podman`.
    #[arg(long, default_value = "docker")]
    engine: String,
}

/// A container's ID or name, by the characters the engines allow in either.
fn reference(value: &str) -> std::result::Result<String, String> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value.starts_with(|c: char| c.is_ascii_alphanumeric())
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if valid {
        Ok(value.to_owned())
    } else {
        Err("not a container's ID or name".to_owned())
    }
}

#[derive(Subcommand)]
pub(crate) enum EngineAction {
    /// Lets the panel act on what runs on the engine.
    Enable { engine: String },
    /// Leaves what runs on the engine alone.
    Disable { engine: String },
    /// How much disk the engine's images, containers, volumes and build
    /// cache take, as `docker system df` reports it.
    Df { engine: String },
    /// What pruning the engine would remove: containers that are not
    /// running, images no container uses, anonymous volumes nothing mounts,
    /// networks no container is on and build cache not in use. With
    /// `--yes`, removes what this preview lists.
    Prune {
        engine: String,
        /// Images a tag still names but no container uses, too.
        #[arg(long)]
        tagged_images: bool,
        /// Volumes a name was given too; they usually hold data someone
        /// meant to keep.
        #[arg(long)]
        named_volumes: bool,
        /// Removes what the preview lists.
        #[arg(long)]
        yes: bool,
    },
}

const PRUNED: &[Column] = &[
    ("KIND", |item| text(&item["kind"])),
    ("NAME", |item| text(&item["name"])),
    ("SIZE", |item| bytes(&item["size_bytes"])),
];

/// One row of `docker system df`: a kind's count, what is in use, size, and
/// what removing the rest would free.
const DISK: &[Column] = &[
    ("TYPE", |row| text(&row["kind"])),
    ("TOTAL", |row| text(&row["use"]["total"])),
    ("ACTIVE", |row| text(&row["use"]["active"])),
    ("SIZE", |row| bytes(&row["use"]["size_bytes"])),
    ("RECLAIMABLE", |row| {
        let reclaimable = &row["use"]["reclaimable_bytes"];
        match (reclaimable.as_f64(), row["use"]["size_bytes"].as_f64()) {
            (Some(part), Some(size)) if size > 0.0 => {
                format!("{} ({:.0}%)", bytes(reclaimable), part / size * 100.0)
            }
            _ => bytes(reclaimable),
        }
    }),
];

const ENGINES: &[Column] = &[
    ("ENGINE", |engine| text(&engine["id"])),
    ("ENABLED", |engine| text(&engine["enabled"])),
    ("STATE", |engine| {
        if engine["reachable"] == true {
            "reachable".into()
        } else {
            format!("unreachable: {}", text(&engine["detail"]))
        }
    }),
    ("VERSION", |engine| text(&engine["version"]["version"])),
    ("CONTAINERS", |engine| {
        if engine["info"].is_object() {
            format!(
                "{} of {} running",
                text(&engine["info"]["running"]),
                text(&engine["info"]["containers"])
            )
        } else {
            "-".into()
        }
    }),
    ("SOCKET", |engine| text(&engine["socket"])),
];

/// A port as `docker ps` writes it, such as `0.0.0.0:8081->80/tcp`.
fn port(port: &Value) -> String {
    let private = format!(
        "{}/{}",
        text(&port["private_port"]),
        text(&port["protocol"])
    );
    match port["public_port"].as_u64() {
        Some(public) => format!("{}:{public}->{private}", text(&port["host_ip"])),
        None => private,
    }
}

const CONTAINERS: &[Column] = &[
    ("NAME", |container| text(&container["names"][0])),
    ("IMAGE", |container| text(&container["image"])),
    ("STATE", |container| text(&container["state"])),
    ("STATUS", |container| text(&container["status"])),
    ("PORTS", |container| {
        let ports: Vec<String> = container["ports"]
            .as_array()
            .into_iter()
            .flatten()
            .map(port)
            .collect();
        ports.join(", ")
    }),
    ("PROJECT", |container| text(&container["compose_project"])),
];

/// Two sizes as `docker stats` writes them, such as `1.0 KiB / 2.0 KiB`.
fn pair(first: &Value, second: &Value) -> String {
    format!("{} / {}", bytes(first), bytes(second))
}

const STATS: &[Column] = &[
    ("NAME", |stats| text(&stats["name"])),
    ("CPU %", |stats| {
        format!("{:.2}%", stats["cpu_percent"].as_f64().unwrap_or(0.0))
    }),
    ("MEM USAGE / LIMIT", |stats| {
        pair(&stats["memory_bytes"], &stats["memory_limit_bytes"])
    }),
    ("MEM %", |stats| {
        match (
            stats["memory_bytes"].as_f64(),
            stats["memory_limit_bytes"].as_f64(),
        ) {
            (Some(used), Some(limit)) if limit > 0.0 => format!("{:.2}%", used / limit * 100.0),
            _ => "-".into(),
        }
    }),
    ("NET I/O", |stats| match &stats["network"] {
        Value::Null => "-".into(),
        network => pair(&network["received_bytes"], &network["sent_bytes"]),
    }),
    ("BLOCK I/O", |stats| {
        pair(&stats["block_read_bytes"], &stats["block_written_bytes"])
    }),
    ("PIDS", |stats| text(&stats["pids"])),
];

const DETAIL: &[Column] = &[
    ("Name", |detail| text(&detail["container"]["names"][0])),
    ("ID", |detail| text(&detail["container"]["id"])),
    ("Image", |detail| text(&detail["container"]["image"])),
    ("State", |detail| {
        let state = text(&detail["container"]["state"]);
        match detail["health"].as_str() {
            Some(health) => format!("{state} ({health})"),
            None => state,
        }
    }),
    ("Status", |detail| text(&detail["container"]["status"])),
    ("Started", |detail| text(&detail["started_at"])),
    ("Stopped", |detail| text(&detail["finished_at"])),
    ("Exit code", |detail| text(&detail["exit_code"])),
    ("Error", |detail| text(&detail["error"])),
    ("Restarts", |detail| text(&detail["restarts"])),
    ("Restart policy", |detail| text(&detail["restart_policy"])),
    ("Host name", |detail| text(&detail["hostname"])),
    ("User", |detail| text(&detail["user"])),
    ("Working directory", |detail| {
        text(&detail["working_directory"])
    }),
    ("Platform", |detail| text(&detail["platform"])),
    ("Project", |detail| {
        text(&detail["container"]["compose_project"])
    }),
];

const LABELS: &[Column] = &[
    ("LABEL", |label| text(&label["name"])),
    ("VALUE", |label| text(&label["value"])),
];

const MOUNTS: &[Column] = &[
    ("TYPE", |mount| text(&mount["kind"])),
    ("SOURCE", |mount| text(&mount["source"])),
    ("DESTINATION", |mount| text(&mount["destination"])),
    ("ACCESS", |mount| {
        if mount["read_write"] == true {
            "read-write".into()
        } else {
            "read-only".into()
        }
    }),
];

const NETWORKS: &[Column] = &[
    ("NETWORK", |network| text(&network["name"])),
    ("ADDRESS", |network| text(&network["ip_address"])),
    ("GATEWAY", |network| text(&network["gateway"])),
    ("ALIASES", |network| {
        let aliases: Vec<String> = network["aliases"]
            .as_array()
            .into_iter()
            .flatten()
            .map(text)
            .collect();
        aliases.join(", ")
    }),
];

pub async fn run(api: &Api, output: &Output, command: ContainerCommand) -> Result<()> {
    match command {
        ContainerCommand::Engines => {
            let engines = api.get("/api/v1/container-engines", &[]).await?.body;
            if output.format == Format::Json {
                output.json(&engines);
            } else if !output.quiet {
                output.list(&engines["engines"], ENGINES);
            }
        }
        ContainerCommand::Engine { action } => match action {
            EngineAction::Enable { engine } => set_engine(api, output, &engine, "enable").await?,
            EngineAction::Disable { engine } => set_engine(api, output, &engine, "disable").await?,
            EngineAction::Df { engine } => disk_usage(api, output, &engine).await?,
            EngineAction::Prune {
                engine,
                tagged_images,
                named_volumes,
                yes,
            } => prune(api, output, &engine, tagged_images, named_volumes, yes).await?,
        },
        ContainerCommand::List {
            engine,
            search,
            states,
        } => {
            let mut query = Vec::new();
            if let Some(search) = search {
                query.push(("search", search));
            }
            if !states.is_empty() {
                query.push(("state", states.join(",")));
            }
            let path = format!("/api/v1/container-engines/{engine}/containers");
            let list = api.get(&path, &query).await?.body;
            if output.format == Format::Json {
                output.json(&list);
            } else if !output.quiet {
                if list["containers"]
                    .as_array()
                    .is_some_and(|containers| containers.is_empty())
                {
                    eprintln!("no containers match");
                } else {
                    output.list(&list["containers"], CONTAINERS);
                }
            }
        }
        ContainerCommand::Inspect { target } => inspect(api, output, &target).await?,
        ContainerCommand::Stats { container, engine } => {
            let path = match &container {
                Some(container) => {
                    format!("/api/v1/container-engines/{engine}/containers/{container}/stats")
                }
                None => format!("/api/v1/container-engines/{engine}/stats"),
            };
            let stats = api.get(&path, &[]).await?.body;
            if output.format == Format::Json {
                output.json(&stats);
            } else if container.is_some() {
                output.list(&Value::Array(vec![stats]), STATS);
            } else if stats["stats"].as_array().is_some_and(Vec::is_empty) {
                if !output.quiet {
                    eprintln!("no containers are running");
                }
            } else {
                output.list(&stats["stats"], STATS);
            }
        }
        ContainerCommand::Logs {
            target,
            lines,
            since,
            follow,
            timestamps,
        } => {
            let path = format!(
                "/api/v1/container-engines/{}/containers/{}/logs",
                target.engine, target.container
            );
            if follow {
                tail(api, output, &path, lines, since, timestamps).await?;
            } else {
                let mut query = vec![("lines", lines.to_string())];
                query.extend(since.map(|since| ("since", since)));
                let logs = api.get(&path, &query).await?.body;
                if output.format == Format::Json {
                    output.json(&logs);
                } else {
                    print(output, &logs["lines"], timestamps)?;
                    if logs["truncated"] == true && !output.quiet {
                        eprintln!("older lines were left out; ask for fewer or use --since");
                    }
                }
            }
        }
        ContainerCommand::Start { target } => act(api, output, &target, "start").await?,
        ContainerCommand::Stop { target, yes } => {
            confirmed(yes)?;
            act(api, output, &target, "stop").await?;
        }
        ContainerCommand::Restart { target, yes } => {
            confirmed(yes)?;
            act(api, output, &target, "restart").await?;
        }
        ContainerCommand::Kill { target, yes } => {
            confirmed(yes)?;
            act(api, output, &target, "kill").await?;
        }
        ContainerCommand::Remove {
            target,
            force,
            volumes,
            yes,
        } => {
            confirmed(yes)?;
            let path = format!(
                "/api/v1/container-engines/{}/containers/{}?force={force}&volumes={volumes}",
                target.engine, target.container
            );
            let change = api.change(Method::DELETE, &path, None, None).await?.body;
            if output.format == Format::Json {
                output.json(&change);
            } else if !output.quiet {
                println!("removed {}", text(&change["name"]));
            }
        }
    }
    Ok(())
}

async fn inspect(api: &Api, output: &Output, target: &Target) -> Result<()> {
    let path = format!(
        "/api/v1/container-engines/{}/containers/{}",
        target.engine, target.container
    );
    let detail = api.get(&path, &[]).await?.body;
    if output.format == Format::Json {
        output.json(&detail);
        return Ok(());
    }
    if output.quiet {
        return Ok(());
    }
    output.item(&detail, DETAIL);
    let labels: Vec<Value> = detail["container"]["labels"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, value)| serde_json::json!({ "name": name, "value": value }))
        .collect();
    for (items, columns) in [
        (Value::Array(labels), LABELS),
        (detail["mounts"].clone(), MOUNTS),
        (detail["networks"].clone(), NETWORKS),
    ] {
        if items.as_array().is_some_and(|items| !items.is_empty()) {
            println!();
            output.list(&items, columns);
        }
    }
    Ok(())
}

fn confirmed(yes: bool) -> Result<()> {
    if yes {
        Ok(())
    } else {
        Err(CliError::Usage(
            "what runs in the container stops; pass --yes to confirm".into(),
        ))
    }
}

async fn act(api: &Api, output: &Output, target: &Target, action: &str) -> Result<()> {
    let path = format!(
        "/api/v1/container-engines/{}/containers/{}/{action}",
        target.engine, target.container
    );
    let change = api.change(Method::POST, &path, None, None).await?.body;
    if output.format == Format::Json {
        output.json(&change);
    } else if !output.quiet {
        output.item(&change["container"], CONTAINERS);
    }
    Ok(())
}

/// Prints lines as the container printed them, its standard error to
/// standard error, or as JSON lines; false once the output is closed.
fn print(output: &Output, lines: &Value, timestamps: bool) -> Result<bool> {
    if output.quiet {
        return Ok(true);
    }
    let (mut stdout, mut stderr) = (std::io::stdout().lock(), std::io::stderr().lock());
    for line in lines.as_array().into_iter().flatten() {
        let sink: &mut dyn Write = if line["stream"] == "stderr" && output.format == Format::Table {
            &mut stderr
        } else {
            &mut stdout
        };
        let text = line["text"].as_str().unwrap_or_default();
        let written = match (output.format, timestamps) {
            (Format::Json, _) => writeln!(sink, "{line}"),
            (Format::Table, true) => {
                writeln!(sink, "{} {text}", line["time"].as_str().unwrap_or_default())
            }
            (Format::Table, false) => writeln!(sink, "{text}"),
        };
        match written {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::BrokenPipe => return Ok(false),
            Err(error) => return Err(CliError::Failed(format!("cannot print: {error}"))),
        }
    }
    match stdout.flush().and_then(|()| stderr.flush()) {
        Err(error) if error.kind() == ErrorKind::BrokenPipe => Ok(false),
        _ => Ok(true),
    }
}

/// Prints the lines a container prints until it stops, the API ends the
/// tail, standard output closes or the user interrupts.
async fn tail(
    api: &Api,
    output: &Output,
    path: &str,
    lines: u32,
    since: Option<String>,
    timestamps: bool,
) -> Result<()> {
    if lines > MOST_BACKLOG {
        return Err(CliError::Usage(format!(
            "--follow shows at most {MOST_BACKLOG} earlier lines"
        )));
    }
    let query = match since {
        Some(since) => vec![("after", since)],
        None => vec![("lines", lines.to_string())],
    };
    let mut socket = api.websocket(&format!("{path}/tail"), &query).await?;
    loop {
        let message = tokio::select! {
            message = socket.next() => message,
            _ = tokio::signal::ctrl_c() => {
                let _ = socket.close(None).await;
                return Ok(());
            }
        };
        let message: Value = match message {
            None | Some(Ok(Message::Close(_))) => return Ok(()),
            Some(Err(error)) => return Err(CliError::transport(error)),
            Some(Ok(Message::Text(message))) => {
                serde_json::from_str(&message).map_err(|error| {
                    CliError::Transport(format!("the API sent an unreadable line: {error}"))
                })?
            }
            Some(Ok(_)) => continue,
        };
        if !print(output, &message["lines"], timestamps)? {
            let _ = socket.close(None).await;
            return Ok(());
        }
        if !message["error"].is_null() {
            return Err(CliError::Ended {
                code: text(&message["error"]["code"]),
                message: text(&message["error"]["message"]),
            });
        }
    }
}

async fn set_engine(api: &Api, output: &Output, engine: &str, verb: &str) -> Result<()> {
    let path = format!("/api/v1/container-engines/{engine}/{verb}");
    let engine = api.change(Method::POST, &path, None, None).await?.body;
    if output.format == Format::Json {
        output.json(&engine);
    } else if !output.quiet {
        output.item(&engine, ENGINES);
    }
    Ok(())
}

async fn disk_usage(api: &Api, output: &Output, engine: &str) -> Result<()> {
    let path = format!("/api/v1/container-engines/{engine}/disk-usage");
    let usage = api.get(&path, &[]).await?.body;
    if output.format == Format::Json {
        output.json(&usage);
        return Ok(());
    }
    let rows: Vec<Value> = [
        ("Images", "images"),
        ("Containers", "containers"),
        ("Local Volumes", "volumes"),
        ("Build Cache", "build_cache"),
    ]
    .into_iter()
    .map(|(kind, field)| serde_json::json!({"kind": kind, "use": usage[field]}))
    .collect();
    output.list(&Value::Array(rows), DISK);
    Ok(())
}

async fn prune(
    api: &Api,
    output: &Output,
    engine: &str,
    tagged_images: bool,
    named_volumes: bool,
    yes: bool,
) -> Result<()> {
    let query = vec![
        ("tagged_images", tagged_images.to_string()),
        ("named_volumes", named_volumes.to_string()),
    ];
    let path = format!("/api/v1/container-engines/{engine}/prune-preview");
    let preview = api.get(&path, &query).await?.body;
    let items = preview["items"].as_array().cloned().unwrap_or_default();
    if !yes {
        if output.format == Format::Json {
            output.json(&preview);
        } else if items.is_empty() {
            eprintln!("nothing to prune");
        } else {
            output.list(&preview["items"], PRUNED);
            if !output.quiet {
                eprintln!(
                    "{} reclaimable; pass --yes to remove these",
                    bytes(&preview["reclaimable_bytes"])
                );
            }
        }
        return Ok(());
    }
    if items.is_empty() {
        if !output.quiet {
            eprintln!("nothing to prune");
        }
        return Ok(());
    }
    let body = serde_json::json!({
        "tagged_images": tagged_images,
        "named_volumes": named_volumes,
        "items": items,
    });
    let path = format!("/api/v1/container-engines/{engine}/prune");
    let report = api
        .change(Method::POST, &path, Some(&body), None)
        .await?
        .body;
    if output.format == Format::Json {
        output.json(&report);
    } else if !output.quiet {
        for outcome in report["outcomes"].as_array().into_iter().flatten() {
            let item = &outcome["item"];
            match &outcome["error"] {
                Value::Null => println!("removed {} {}", text(&item["kind"]), text(&item["name"])),
                error => println!(
                    "kept {} {}: {}",
                    text(&item["kind"]),
                    text(&item["name"]),
                    text(&error["message"])
                ),
            }
        }
        println!("{} reclaimed", bytes(&report["reclaimed_bytes"]));
    }
    Ok(())
}
