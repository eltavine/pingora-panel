//! The container engines the host agent reaches and what runs on them.

use crate::{
    client::{Api, CliError, Result},
    output::{text, Column, Format, Output},
};
use clap::Subcommand;
use reqwest::Method;
use serde_json::Value;

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
}

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
        ContainerCommand::Engine { action } => {
            let (engine, verb) = match action {
                EngineAction::Enable { engine } => (engine, "enable"),
                EngineAction::Disable { engine } => (engine, "disable"),
            };
            let path = format!("/api/v1/container-engines/{engine}/{verb}");
            let engine = api.change(Method::POST, &path, None, None).await?.body;
            if output.format == Format::Json {
                output.json(&engine);
            } else if !output.quiet {
                output.item(&engine, ENGINES);
            }
        }
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
