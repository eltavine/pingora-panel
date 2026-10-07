//! External plugins (ADR 0044): signed packages found in the plugins
//! directory, granted capabilities, configured against their JSON Schema
//! with secrets named by reference, and run as child processes under
//! limits.

use crate::{
    client::{Api, CliError, Result},
    commands::{read_file, read_json},
    output::{bytes, text, Column, Format, Output},
};
use clap::{Args, Subcommand};
use reqwest::Method;
use serde_json::{json, Map, Value};
use std::{
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

/// Long enough for a plugin to start, shake hands and apply its settings.
const STARTING: Duration = Duration::from_secs(90);

#[derive(Subcommand)]
pub(crate) enum PluginCommand {
    /// Plugins found or configured, and what the host offers them.
    List,
    /// A plugin: its versions, grants, settings, limits and health.
    Show { name: String },
    /// The versions of a plugin found, with their signatures and problems.
    Versions { name: String },
    /// Whether a running plugin answers its health checks.
    Health { name: String },
    /// Reads the plugins directory again.
    Discover,
    /// Grants capabilities, in place of those granted before; none revokes
    /// every grant.
    Grant {
        name: String,
        /// Such as `dns01,secret-references`.
        #[arg(value_delimiter = ',')]
        capabilities: Vec<String>,
        #[command(flatten)]
        condition: Condition,
    },
    /// Replaces the settings, or changes some of them with --set.
    Configure {
        name: String,
        /// A JSON object of every setting, from a file or `-` for standard
        /// input.
        #[arg(long, value_name = "FILE", conflicts_with_all = ["set", "unset"])]
        settings: Option<String>,
        /// KEY=VALUE, merged into the current settings; VALUE is JSON when
        /// it reads as JSON and text otherwise, so `token=vault:dns` names
        /// a kept secret. Repeat it for more.
        #[arg(long, value_name = "KEY=VALUE")]
        set: Vec<String>,
        /// A setting to remove; repeat it for more.
        #[arg(long, value_name = "KEY")]
        unset: Vec<String>,
        #[command(flatten)]
        condition: Condition,
    },
    /// Sets resource limits; 0 leaves one to the manifest or the host.
    Limit {
        name: String,
        #[arg(long, default_value_t = 0)]
        memory_bytes: u64,
        #[arg(long, default_value_t = 0)]
        cpu_seconds: u32,
        #[arg(long, default_value_t = 0)]
        open_files: u32,
        /// Calls served at once.
        #[arg(long, default_value_t = 0)]
        concurrency: u32,
        /// The longest a call may take, in milliseconds.
        #[arg(long, default_value_t = 0)]
        call_timeout_ms: u32,
        #[command(flatten)]
        condition: Condition,
    },
    /// Starts the plugin at a version, or its newest validated one.
    Enable {
        name: String,
        #[arg(long)]
        version: Option<String>,
        #[command(flatten)]
        condition: Condition,
    },
    /// Stops the plugin, keeping its grants, settings and versions.
    Disable {
        name: String,
        #[command(flatten)]
        condition: Condition,
    },
    /// Runs another validated version in place of the active one.
    Upgrade {
        name: String,
        version: String,
        #[command(flatten)]
        condition: Condition,
    },
    /// Runs the version that ran before the active one.
    Rollback {
        name: String,
        #[command(flatten)]
        condition: Condition,
    },
    /// The publisher keys whose signatures the host accepts.
    #[command(subcommand)]
    Key(KeyCommand),
    /// Secrets kept for plugins' settings, named as `vault:<name>`.
    #[command(subcommand)]
    Secret(SecretCommand),
}

#[derive(Args)]
pub(crate) struct Condition {
    /// The ETag the plugin must still have.
    #[arg(long, value_name = "ETAG")]
    if_match: Option<String>,
}

#[derive(Subcommand)]
pub(crate) enum KeyCommand {
    List,
    /// Trusts a publisher's minisign public key.
    Add {
        id: String,
        /// The key file, as `minisign -G` writes it, or `-` for standard
        /// input.
        #[arg(long, value_name = "FILE", required_unless_present = "public_key")]
        file: Option<String>,
        /// The key's base64 line.
        #[arg(long, conflicts_with = "file")]
        public_key: Option<String>,
        #[arg(long)]
        comment: Option<String>,
    },
    /// No longer trusts a key; refused while it signs an enabled plugin.
    Rm {
        id: String,
        /// Confirms the removal.
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub(crate) enum SecretCommand {
    /// The secrets' names; their values are never shown.
    List,
    /// Seals a secret, read from a file or `-` for standard input.
    Set {
        name: String,
        #[arg(long, value_name = "FILE")]
        from: PathBuf,
    },
    /// Deletes a secret no plugin's settings name.
    Rm {
        name: String,
        /// Confirms the removal.
        #[arg(long)]
        yes: bool,
    },
}

fn joined(values: &Value) -> String {
    values
        .as_array()
        .map(|values| values.iter().map(text).collect::<Vec<_>>().join(","))
        .unwrap_or_default()
}

fn health(plugin: &Value) -> String {
    match plugin["health"]["status"].as_str() {
        Some(status) => status.to_owned(),
        None => text(&plugin["error"]),
    }
}

fn limits(limits: &Value) -> String {
    if limits.is_null() {
        return String::new();
    }
    format!(
        "memory {}, open files {}, {} calls at once, {} ms a call{}",
        bytes(&limits["memory_bytes"]),
        text(&limits["open_files"]),
        text(&limits["concurrency"]),
        text(&limits["call_timeout_ms"]),
        match limits["cpu_seconds"].as_u64() {
            Some(seconds) if seconds > 0 => format!(", {seconds} s of CPU"),
            _ => String::new(),
        }
    )
}

const PLUGINS: &[Column] = &[
    ("NAME", |plugin| text(&plugin["name"])),
    ("STATE", |plugin| text(&plugin["state"])),
    ("VERSION", |plugin| text(&plugin["active_version"])),
    ("VERSIONS", |plugin| {
        plugin["versions"]
            .as_array()
            .map(|versions| {
                versions
                    .iter()
                    .map(|version| text(&version["version"]))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default()
    }),
    ("GRANTS", |plugin| joined(&plugin["grants"])),
    ("HEALTH", health),
];

const PLUGIN: &[Column] = &[
    ("Name", |plugin| text(&plugin["name"])),
    ("State", |plugin| text(&plugin["state"])),
    ("Version", |plugin| text(&plugin["active_version"])),
    ("Previous", |plugin| text(&plugin["previous_version"])),
    ("Grants", |plugin| joined(&plugin["grants"])),
    ("Settings", |plugin| plugin["settings"].to_string()),
    ("Limits", |plugin| limits(&plugin["effective_limits"])),
    ("Health", health),
    ("Updated", |plugin| text(&plugin["updated_at"])),
    ("ETag", |plugin| text(&plugin["etag"])),
];

const VERSIONS: &[Column] = &[
    ("VERSION", |version| text(&version["version"])),
    ("PUBLISHER", |version| text(&version["publisher"])),
    ("SIGNED BY", |version| text(&version["signed_by"])),
    ("PROTOCOL", |version| joined(&version["protocol_versions"])),
    ("PORTS", |version| joined(&version["ports"])),
    ("PROBLEMS", |version| {
        version["problems"]
            .as_array()
            .map(|problems| problems.iter().map(text).collect::<Vec<_>>().join("; "))
            .unwrap_or_default()
    }),
];

const HEALTH: &[Column] = &[
    ("State", |plugin| text(&plugin["state"])),
    ("Version", |plugin| text(&plugin["health"]["version"])),
    ("Status", |plugin| text(&plugin["health"]["status"])),
    ("Started", |plugin| text(&plugin["health"]["started_at"])),
    ("Checked", |plugin| text(&plugin["health"]["checked_at"])),
    ("Failures", |plugin| text(&plugin["health"]["failures"])),
    ("Restarts", |plugin| text(&plugin["health"]["restarts"])),
    ("Error", |plugin| {
        let error = text(&plugin["health"]["error"]);
        if error.is_empty() {
            text(&plugin["error"])
        } else {
            error
        }
    }),
];

const KEYS: &[Column] = &[
    ("ID", |key| text(&key["id"])),
    ("KEY ID", |key| text(&key["key_id"])),
    ("COMMENT", |key| text(&key["comment"])),
    ("ADDED", |key| text(&key["created_at"])),
];

const SECRETS: &[Column] = &[
    ("NAME", |secret| text(&secret["name"])),
    ("UPDATED", |secret| text(&secret["updated_at"])),
];

fn path(name: &str) -> String {
    format!("/api/v1/plugins/{name}")
}

/// A setting as `--set` gives it: JSON when it reads as JSON, else text.
fn setting(entry: &str) -> Result<(String, Value)> {
    let (key, value) = entry
        .split_once('=')
        .filter(|(key, _)| !key.is_empty())
        .ok_or_else(|| CliError::Usage(format!("--set {entry:?} is not KEY=VALUE")))?;
    let value = serde_json::from_str(value).unwrap_or_else(|_| json!(value));
    Ok((key.to_owned(), value))
}

/// A secret's value from a file, or from standard input for `-`; one final
/// line break is not part of it.
fn secret(from: &Path) -> Result<String> {
    let value = if from.as_os_str() == "-" {
        let mut value = String::new();
        std::io::stdin()
            .read_to_string(&mut value)
            .map_err(|error| CliError::Usage(format!("cannot read standard input: {error}")))?;
        value
    } else {
        read_file(from)?
    };
    let value = value
        .strip_suffix('\n')
        .map(|value| value.strip_suffix('\r').unwrap_or(value))
        .unwrap_or(&value)
        .to_owned();
    if value.is_empty() {
        return Err(CliError::Usage("the secret is empty".into()));
    }
    Ok(value)
}

async fn change(
    api: &Api,
    output: &Output,
    method: Method,
    path: &str,
    body: Option<&Value>,
    condition: &Condition,
    done: &str,
) -> Result<()> {
    let reply = api
        .change_lasting(method, path, body, condition.if_match.as_deref(), STARTING)
        .await?;
    if output.format == Format::Json {
        output.json(&reply.body);
    } else {
        output.done(done, &reply.body);
        output.item(&reply.body, PLUGIN);
    }
    Ok(())
}

pub(crate) async fn run(api: &Api, output: &Output, command: PluginCommand) -> Result<()> {
    match command {
        PluginCommand::List => {
            let listed = api.get("/api/v1/plugins", &[]).await?.body;
            if output.format == Format::Json {
                output.json(&listed);
            } else if listed["plugins"].as_array().is_some_and(Vec::is_empty) {
                if !output.quiet {
                    eprintln!(
                        "no plugins were found; protocol versions {} and ports {} are offered",
                        joined(&listed["protocol_versions"]),
                        joined(&listed["ports"])
                    );
                }
            } else {
                output.list(&listed["plugins"], PLUGINS);
            }
        }
        PluginCommand::Show { name } => {
            let plugin = api.get(&path(&name), &[]).await?.body;
            output.item(&plugin, PLUGIN);
        }
        PluginCommand::Versions { name } => {
            let plugin = api.get(&path(&name), &[]).await?.body;
            if output.format == Format::Json {
                output.json(&plugin["versions"]);
            } else {
                output.list(&plugin["versions"], VERSIONS);
            }
        }
        PluginCommand::Health { name } => {
            let plugin = api.get(&path(&name), &[]).await?.body;
            if output.format == Format::Json {
                output.json(&json!({
                    "state": plugin["state"],
                    "health": plugin["health"],
                    "error": plugin["error"],
                }));
            } else {
                output.item(&plugin, HEALTH);
            }
        }
        PluginCommand::Discover => {
            let listed = api
                .change(Method::POST, "/api/v1/plugins/discover", None, None)
                .await?
                .body;
            if output.format == Format::Json {
                output.json(&listed);
            } else {
                output.list(&listed["plugins"], PLUGINS);
            }
        }
        PluginCommand::Grant {
            name,
            capabilities,
            condition,
        } => {
            let body = json!({ "capabilities": capabilities });
            let uri = format!("{}/grants", path(&name));
            change(
                api,
                output,
                Method::PUT,
                &uri,
                Some(&body),
                &condition,
                "granted",
            )
            .await?;
        }
        PluginCommand::Configure {
            name,
            settings,
            set,
            unset,
            condition,
        } => {
            let (settings, if_match) = match settings {
                Some(file) => (read_json(&file)?, condition.if_match),
                None if set.is_empty() && unset.is_empty() => {
                    return Err(CliError::Usage(
                        "give --settings, or --set and --unset to change some settings".into(),
                    ));
                }
                None => {
                    let current = api.get(&path(&name), &[]).await?;
                    let mut settings = match current.body["settings"].clone() {
                        Value::Object(settings) => settings,
                        _ => Map::new(),
                    };
                    for key in unset {
                        settings.remove(&key);
                    }
                    for entry in set {
                        let (key, value) = setting(&entry)?;
                        settings.insert(key, value);
                    }
                    let tag = condition.if_match.or(current.etag);
                    (Value::Object(settings), tag)
                }
            };
            let uri = format!("{}/settings", path(&name));
            let condition = Condition { if_match };
            change(
                api,
                output,
                Method::PUT,
                &uri,
                Some(&settings),
                &condition,
                "configured",
            )
            .await?;
        }
        PluginCommand::Limit {
            name,
            memory_bytes,
            cpu_seconds,
            open_files,
            concurrency,
            call_timeout_ms,
            condition,
        } => {
            let body = json!({
                "memory_bytes": memory_bytes,
                "cpu_seconds": cpu_seconds,
                "open_files": open_files,
                "concurrency": concurrency,
                "call_timeout_ms": call_timeout_ms,
            });
            let uri = format!("{}/limits", path(&name));
            change(
                api,
                output,
                Method::PUT,
                &uri,
                Some(&body),
                &condition,
                "limited",
            )
            .await?;
        }
        PluginCommand::Enable {
            name,
            version,
            condition,
        } => {
            let body = json!({ "version": version });
            let uri = format!("{}/enable", path(&name));
            change(
                api,
                output,
                Method::POST,
                &uri,
                Some(&body),
                &condition,
                "enabled",
            )
            .await?;
        }
        PluginCommand::Disable { name, condition } => {
            let uri = format!("{}/disable", path(&name));
            change(
                api,
                output,
                Method::POST,
                &uri,
                None,
                &condition,
                "disabled",
            )
            .await?;
        }
        PluginCommand::Upgrade {
            name,
            version,
            condition,
        } => {
            let body = json!({ "version": version });
            let uri = format!("{}/upgrade", path(&name));
            change(
                api,
                output,
                Method::POST,
                &uri,
                Some(&body),
                &condition,
                "upgraded",
            )
            .await?;
        }
        PluginCommand::Rollback { name, condition } => {
            let uri = format!("{}/rollback", path(&name));
            change(
                api,
                output,
                Method::POST,
                &uri,
                None,
                &condition,
                "rolled back",
            )
            .await?;
        }
        PluginCommand::Key(KeyCommand::List) => {
            let keys = api.get("/api/v1/plugin-keys", &[]).await?.body;
            output.list(&keys, KEYS);
        }
        PluginCommand::Key(KeyCommand::Add {
            id,
            file,
            public_key,
            comment,
        }) => {
            let public_key = match (file, public_key) {
                (_, Some(key)) => key,
                (Some(file), None) if file == "-" => std::io::read_to_string(std::io::stdin())
                    .map_err(|error| {
                        CliError::Usage(format!("cannot read standard input: {error}"))
                    })?,
                (Some(file), None) => read_file(&PathBuf::from(file))?,
                (None, None) => return Err(CliError::Usage("give --file or --public-key".into())),
            };
            let body = json!({
                "id": id,
                "public_key": public_key,
                "comment": comment.unwrap_or_default(),
            });
            let reply = api
                .change(Method::POST, "/api/v1/plugin-keys", Some(&body), None)
                .await?;
            output.done("trusted", &reply.body);
            output.item(&reply.body, KEYS);
        }
        PluginCommand::Key(KeyCommand::Rm { id, yes }) => {
            if !yes {
                return Err(CliError::Usage(format!(
                    "plugins signed by key {id} will no longer validate; confirm with --yes"
                )));
            }
            let reply = api
                .change(
                    Method::DELETE,
                    &format!("/api/v1/plugin-keys/{id}"),
                    None,
                    None,
                )
                .await?;
            output.done("removed", &reply.body);
        }
        PluginCommand::Secret(SecretCommand::List) => {
            let secrets = api.get("/api/v1/plugin-secrets", &[]).await?.body;
            output.list(&secrets, SECRETS);
        }
        PluginCommand::Secret(SecretCommand::Set { name, from }) => {
            let body = json!({ "value": secret(&from)? });
            let reply = api
                .change(
                    Method::PUT,
                    &format!("/api/v1/plugin-secrets/{name}"),
                    Some(&body),
                    None,
                )
                .await?;
            output.done("sealed", &reply.body);
            output.item(&reply.body, SECRETS);
        }
        PluginCommand::Secret(SecretCommand::Rm { name, yes }) => {
            if !yes {
                return Err(CliError::Usage(format!(
                    "secret {name} cannot be recovered once deleted; confirm with --yes"
                )));
            }
            let reply = api
                .change(
                    Method::DELETE,
                    &format!("/api/v1/plugin-secrets/{name}"),
                    None,
                    None,
                )
                .await?;
            output.done("deleted", &reply.body);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_on_the_command_line_are_json_or_text() {
        assert_eq!(
            setting("delay_ms=250").unwrap(),
            ("delay_ms".into(), json!(250))
        );
        assert_eq!(
            setting("token=vault:dns").unwrap(),
            ("token".into(), json!("vault:dns"))
        );
        assert_eq!(
            setting("zones=[\"example.com\"]").unwrap(),
            ("zones".into(), json!(["example.com"]))
        );
        assert_eq!(setting("empty=").unwrap(), ("empty".into(), json!("")));
        assert!(setting("=1").is_err());
        assert!(setting("token").is_err());
    }

    #[test]
    fn limits_read_as_a_sentence() {
        assert_eq!(limits(&Value::Null), "");
        let described = limits(&json!({
            "memory_bytes": 536870912u64, "open_files": 256, "concurrency": 8,
            "call_timeout_ms": 10000, "cpu_seconds": 0
        }));
        assert!(described.starts_with("memory "), "{described}");
        assert!(described.ends_with("10000 ms a call"), "{described}");
    }
}
