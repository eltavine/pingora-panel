//! What the container engines the host agent reaches keep besides
//! containers and images: their networks and volumes.

use crate::{
    client::{Api, Result},
    output::{text, Column, Format, Output},
};
use clap::Subcommand;
use serde_json::Value;

#[derive(Subcommand)]
pub(crate) enum NetworkCommand {
    /// An engine's networks, with their subnets and the containers attached
    /// to each.
    List {
        /// `docker` or `podman`.
        #[arg(long, default_value = "docker")]
        engine: String,
    },
}

#[derive(Subcommand)]
pub(crate) enum VolumeCommand {
    /// An engine's volumes, with where their data lives and the containers
    /// that mount each.
    List {
        /// `docker` or `podman`.
        #[arg(long, default_value = "docker")]
        engine: String,
    },
}

/// Subnets as `172.18.0.0/16 via 172.18.0.1`.
fn subnets(network: &Value) -> String {
    let subnets: Vec<String> = network["subnets"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|subnet| match subnet["gateway"].as_str() {
            Some(gateway) => format!("{} via {gateway}", text(&subnet["subnet"])),
            None => text(&subnet["subnet"]),
        })
        .collect();
    if subnets.is_empty() {
        "-".into()
    } else {
        subnets.join(", ")
    }
}

const NETWORKS: &[Column] = &[
    ("NAME", |network| text(&network["name"])),
    ("DRIVER", |network| text(&network["driver"])),
    ("SCOPE", |network| text(&network["scope"])),
    ("SUBNETS", subnets),
    ("CONTAINERS", |network| text(&network["containers"])),
    ("PROJECT", |network| text(&network["compose_project"])),
];

const VOLUMES: &[Column] = &[
    ("NAME", |volume| text(&volume["name"])),
    ("DRIVER", |volume| text(&volume["driver"])),
    ("CONTAINERS", |volume| text(&volume["containers"])),
    ("PROJECT", |volume| text(&volume["compose_project"])),
    ("MOUNTPOINT", |volume| text(&volume["mountpoint"])),
];

async fn list(
    api: &Api,
    output: &Output,
    path: String,
    items: &str,
    columns: &[Column],
) -> Result<()> {
    let list = api.get(&path, &[]).await?.body;
    if output.format == Format::Json {
        output.json(&list);
    } else if list[items].as_array().is_some_and(Vec::is_empty) {
        if !output.quiet {
            eprintln!("the engine has no {items}");
        }
    } else {
        output.list(&list[items], columns);
    }
    Ok(())
}

pub async fn networks(api: &Api, output: &Output, command: NetworkCommand) -> Result<()> {
    match command {
        NetworkCommand::List { engine } => {
            let path = format!("/api/v1/container-engines/{engine}/networks");
            list(api, output, path, "networks", NETWORKS).await
        }
    }
}

pub async fn volumes(api: &Api, output: &Output, command: VolumeCommand) -> Result<()> {
    match command {
        VolumeCommand::List { engine } => {
            let path = format!("/api/v1/container-engines/{engine}/volumes");
            list(api, output, path, "volumes", VOLUMES).await
        }
    }
}
