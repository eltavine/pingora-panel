use super::read_json;
use crate::{
    client::{Api, CliError, Result},
    output::{text, Column, Output},
};
use clap::Subcommand;
use reqwest::Method;
use serde_json::{json, Map, Value};

#[derive(Subcommand)]
pub(crate) enum UpstreamCommand {
    /// Lists upstreams with their nodes and the sites using them.
    List,
    /// Shows an upstream with its nodes and policies.
    Show { id: String },
    /// Creates an upstream from flags, or from a JSON document with --file.
    Create {
        #[arg(long, conflicts_with = "name")]
        file: Option<String>,
        #[arg(long, required_unless_present = "file")]
        name: Option<String>,
        /// HOST:PORT with optional `,weight=N`, `,backup` and `,tls`; repeatable.
        #[arg(long = "node")]
        nodes: Vec<String>,
        /// round_robin, random, or hash with --hash-key.
        #[arg(long, default_value = "round_robin")]
        balancing: String,
        /// client_ip, uri, header:<name> or cookie:<name>.
        #[arg(long)]
        hash_key: Option<String>,
        /// Replaces the client's Host when forwarding.
        #[arg(long)]
        host_header: Option<String>,
        /// Probe this path every --health-interval milliseconds.
        #[arg(long)]
        health_path: Option<String>,
        #[arg(long, default_value_t = 5000)]
        health_interval: u64,
        #[arg(long)]
        note: Option<String>,
    },
    /// Replaces an upstream with a JSON document.
    Update {
        id: String,
        #[arg(long)]
        file: String,
    },
    /// Removes an upstream no site or route uses.
    Delete { id: String },
    /// Nodes of an upstream.
    #[command(subcommand)]
    Node(NodeCommand),
    /// Live health, load and latency of every node.
    Health {
        /// Only this upstream.
        id: Option<String>,
    },
    /// Takes a node out of rotation until restored, across restarts.
    Drain { id: String, node: String },
    /// Returns a drained node to rotation.
    Undrain { id: String, node: String },
}

#[derive(Subcommand)]
pub(crate) enum NodeCommand {
    /// Adds a node to the upstream.
    Add {
        id: String,
        /// HOST:PORT with optional `,weight=N`, `,backup` and `,tls`.
        node: String,
        #[arg(long)]
        sni: Option<String>,
        #[arg(long)]
        note: Option<String>,
    },
    /// Changes a node's address, weight, role or state.
    Update {
        id: String,
        node: String,
        #[arg(long)]
        weight: Option<u32>,
        #[arg(long, conflicts_with = "disable")]
        enable: bool,
        #[arg(long)]
        disable: bool,
        #[arg(long, conflicts_with = "primary")]
        backup: bool,
        #[arg(long)]
        primary: bool,
        #[arg(long)]
        note: Option<String>,
    },
    /// Removes a node from the upstream.
    Remove { id: String, node: String },
}

const COLUMNS: &[Column] = &[
    ("ID", |upstream| text(&upstream["id"])),
    ("NAME", |upstream| text(&upstream["name"])),
    ("BALANCING", |upstream| match &upstream["balancing"] {
        Value::Object(hash) => format!("hash:{}", text(&hash["consistent_hash"]["key"])),
        other => text(other),
    }),
    ("NODES", |upstream| {
        upstream["nodes"].as_array().map_or(0, Vec::len).to_string()
    }),
    ("USED BY", |upstream| {
        upstream["used_by"]
            .as_array()
            .map_or(0, Vec::len)
            .to_string()
    }),
];

const NODES: &[Column] = &[
    ("NODE", |node| text(&node["id"])),
    ("ADDRESS", |node| {
        format!("{}:{}", text(&node["host"]), text(&node["port"]))
    }),
    ("WEIGHT", |node| text(&node["weight"])),
    ("ENABLED", |node| text(&node["enabled"])),
    ("BACKUP", |node| text(&node["backup"])),
    ("TLS", |node| text(&node["tls"])),
    ("NOTE", |node| text(&node["note"])),
];

const HEALTH: &[Column] = &[
    ("UPSTREAM", |row| text(&row["upstream_id"])),
    ("NODE", |row| text(&row["node_id"])),
    ("ADDRESS", |row| text(&row["address"])),
    ("HEALTHY", |row| text(&row["healthy"])),
    ("DRAINED", |row| text(&row["drained"])),
    ("EJECTED UNTIL", |row| text(&row["ejected_until"])),
    ("IN FLIGHT", |row| text(&row["in_flight"])),
    ("REQUESTS", |row| text(&row["requests"])),
    ("FAILURES", |row| text(&row["failures"])),
    ("LATENCY MS", |row| {
        row["latency_us"].as_u64().map_or_else(
            || "-".into(),
            |micros| format!("{:.1}", micros as f64 / 1000.0),
        )
    }),
];

/// `HOST:PORT[,weight=N][,backup][,tls]`; IPv6 hosts use brackets.
fn node(value: &str) -> Result<Value> {
    let mut parts = value.split(',');
    let address = parts.next().unwrap_or_default();
    let (host, port) = address
        .rsplit_once(':')
        .ok_or_else(|| CliError::Usage(format!("{address:?} is not HOST:PORT")))?;
    let port: u16 = port
        .parse()
        .map_err(|_| CliError::Usage(format!("{port:?} is not a port")))?;
    let mut node = Map::new();
    node.insert(
        "host".into(),
        json!(host.trim_start_matches('[').trim_end_matches(']')),
    );
    node.insert("port".into(), json!(port));
    for option in parts {
        match option.split_once('=') {
            Some(("weight", weight)) => {
                let weight: u32 = weight
                    .parse()
                    .map_err(|_| CliError::Usage(format!("{weight:?} is not a weight")))?;
                node.insert("weight".into(), json!(weight));
            }
            None if option == "backup" => {
                node.insert("backup".into(), json!(true));
            }
            None if option == "tls" => {
                node.insert("tls".into(), json!(true));
            }
            _ => return Err(CliError::Usage(format!("unknown node option {option:?}"))),
        }
    }
    Ok(Value::Object(node))
}

fn balancing(policy: &str, key: Option<String>) -> Result<Value> {
    match (policy, key) {
        ("round_robin" | "random", None) => Ok(json!(policy)),
        ("hash", Some(key)) => Ok(json!({ "consistent_hash": { "key": key } })),
        ("hash", None) => Err(CliError::Usage("hash balancing needs --hash-key".into())),
        (_, Some(_)) => Err(CliError::Usage(
            "--hash-key applies to hash balancing".into(),
        )),
        (other, None) => Err(CliError::Usage(format!("unknown balancing {other:?}"))),
    }
}

/// An upstream representation reduced to what clients may write.
fn input(upstream: &Value) -> Value {
    let mut upstream = upstream.clone();
    if let Some(object) = upstream.as_object_mut() {
        for field in ["id", "etag", "used_by", "created_at", "updated_at"] {
            object.remove(field);
        }
    }
    upstream
}

pub async fn run(api: &Api, output: &Output, command: UpstreamCommand) -> Result<()> {
    match command {
        UpstreamCommand::List => {
            output.list(&api.get("/api/v1/upstreams", &[]).await?.body, COLUMNS)
        }
        UpstreamCommand::Show { id } => {
            let upstream = api.get(&format!("/api/v1/upstreams/{id}"), &[]).await?.body;
            output.item(&upstream, COLUMNS);
            if output.format == crate::output::Format::Table && !output.quiet {
                println!();
                output.list(&upstream["nodes"], NODES);
            }
        }
        UpstreamCommand::Create {
            file,
            name,
            nodes,
            balancing: policy,
            hash_key,
            host_header,
            health_path,
            health_interval,
            note,
        } => {
            let body = match file {
                Some(file) => read_json(&file)?,
                None => json!({
                    "name": name,
                    "nodes": nodes.iter().map(|value| node(value)).collect::<Result<Vec<_>>>()?,
                    "balancing": balancing(&policy, hash_key)?,
                    "host_header": host_header,
                    "health_check": health_path.map(|path| json!({
                        "protocol": "http",
                        "path": path,
                        "method": "GET",
                        "interval_ms": health_interval,
                        "timeout_ms": (health_interval / 2).max(100),
                        "healthy_threshold": 2,
                        "unhealthy_threshold": 3,
                    })),
                    "note": note,
                }),
            };
            let upstream = api
                .change(Method::POST, "/api/v1/upstreams", Some(&body), None)
                .await?
                .body;
            output.done(
                &format!(
                    "Created upstream {} ({})",
                    text(&upstream["name"]),
                    text(&upstream["id"])
                ),
                &upstream,
            );
        }
        UpstreamCommand::Update { id, file } => {
            let body = read_json(&file)?;
            let current = api.get(&format!("/api/v1/upstreams/{id}"), &[]).await?;
            let upstream = api
                .change(
                    Method::PUT,
                    &format!("/api/v1/upstreams/{id}"),
                    Some(&input(&body)),
                    current.etag.as_deref(),
                )
                .await?
                .body;
            output.done(
                &format!("Updated upstream {}", text(&upstream["name"])),
                &upstream,
            );
        }
        UpstreamCommand::Delete { id } => {
            let current = api.get(&format!("/api/v1/upstreams/{id}"), &[]).await?;
            let reply = api
                .change(
                    Method::DELETE,
                    &format!("/api/v1/upstreams/{id}"),
                    None,
                    current.etag.as_deref(),
                )
                .await?
                .body;
            output.done(
                &format!("Deleted upstream {}", text(&current.body["name"])),
                &reply,
            );
        }
        UpstreamCommand::Node(command) => nodes(api, output, command).await?,
        UpstreamCommand::Health { id } => {
            let report = api.get("/api/v1/upstreams/health", &[]).await?.body;
            let rows: Vec<Value> = report["upstreams"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|upstream| id.as_deref().is_none_or(|id| upstream["upstream_id"] == id))
                .flat_map(|upstream| {
                    upstream["nodes"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(move |node| {
                            let mut row = node.clone();
                            row["upstream_id"] = upstream["upstream_id"].clone();
                            row
                        })
                })
                .collect();
            if output.format == crate::output::Format::Json {
                output.json(&report);
            } else {
                output.list(&Value::Array(rows), HEALTH);
            }
        }
        UpstreamCommand::Drain { id, node } => drain(api, output, &id, &node, true).await?,
        UpstreamCommand::Undrain { id, node } => drain(api, output, &id, &node, false).await?,
    }
    Ok(())
}

async fn drain(api: &Api, output: &Output, id: &str, node: &str, drained: bool) -> Result<()> {
    let method = if drained { Method::PUT } else { Method::DELETE };
    let health = api
        .change(
            method,
            &format!("/api/v1/upstreams/{id}/nodes/{node}/drain"),
            None,
            None,
        )
        .await?
        .body;
    let verb = if drained { "Drained" } else { "Restored" };
    output.done(&format!("{verb} node {node}"), &health);
    Ok(())
}

async fn nodes(api: &Api, output: &Output, command: NodeCommand) -> Result<()> {
    match command {
        NodeCommand::Add {
            id,
            node: value,
            sni,
            note,
        } => {
            let mut body = node(&value)?;
            body["sni"] = json!(sni);
            body["note"] = json!(note);
            let upstream = api
                .change(
                    Method::POST,
                    &format!("/api/v1/upstreams/{id}/nodes"),
                    Some(&body),
                    None,
                )
                .await?
                .body;
            output.done(
                &format!("Added {value} to {}", text(&upstream["name"])),
                &upstream,
            );
        }
        NodeCommand::Update {
            id,
            node,
            weight,
            enable,
            disable,
            backup,
            primary,
            note,
        } => {
            let current = api.get(&format!("/api/v1/upstreams/{id}"), &[]).await?;
            let mut body = current.body["nodes"]
                .as_array()
                .and_then(|nodes| {
                    nodes
                        .iter()
                        .find(|candidate| candidate["id"] == node.as_str())
                })
                .cloned()
                .ok_or_else(|| {
                    CliError::Usage(format!("node {node} is not part of this upstream"))
                })?;
            if let Some(weight) = weight {
                body["weight"] = json!(weight);
            }
            if enable || disable {
                body["enabled"] = json!(enable);
            }
            if backup || primary {
                body["backup"] = json!(backup);
            }
            if let Some(note) = note {
                body["note"] = json!(note);
            }
            let upstream = api
                .change(
                    Method::PUT,
                    &format!("/api/v1/upstreams/{id}/nodes/{node}"),
                    Some(&body),
                    current.etag.as_deref(),
                )
                .await?
                .body;
            output.done(&format!("Updated node {node}"), &upstream);
        }
        NodeCommand::Remove { id, node } => {
            let current = api.get(&format!("/api/v1/upstreams/{id}"), &[]).await?;
            let upstream = api
                .change(
                    Method::DELETE,
                    &format!("/api/v1/upstreams/{id}/nodes/{node}"),
                    None,
                    current.etag.as_deref(),
                )
                .await?
                .body;
            output.done(&format!("Removed node {node}"), &upstream);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_specs_and_balancing_parse() {
        assert_eq!(
            node("10.0.0.1:8080,weight=3,backup,tls").unwrap(),
            json!({"host": "10.0.0.1", "port": 8080, "weight": 3, "backup": true, "tls": true})
        );
        assert_eq!(node("[::1]:80").unwrap()["host"], "::1");
        assert!(node("host").is_err());
        assert!(node("host:80,fast").is_err());
        assert_eq!(balancing("random", None).unwrap(), json!("random"));
        assert_eq!(
            balancing("hash", Some("cookie:session".into())).unwrap(),
            json!({"consistent_hash": {"key": "cookie:session"}})
        );
        assert!(balancing("hash", None).is_err());
    }
}
