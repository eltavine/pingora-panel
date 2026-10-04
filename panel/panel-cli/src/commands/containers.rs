//! The container engines the host agent reaches and what runs on them.

use crate::{
    client::{Api, Result},
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
    }
    Ok(())
}
