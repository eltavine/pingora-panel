//! Alert rules, the channels they notify and the notifications sent.

use super::read_file;
use crate::{
    client::{Api, CliError, Result},
    output::{text, Column, Format, Output},
};
use clap::{Args, Subcommand, ValueEnum};
use reqwest::Method;
use serde_json::{json, Value};
use std::{
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

const RULES: &str = "/api/v1/alert-rules";
const CHANNELS: &str = "/api/v1/alert-channels";

#[derive(Clone, Copy, ValueEnum)]
enum MeasureName {
    /// The share of requests answered with a 5xx status, from 0 to 1.
    ServerErrorRatio,
    /// The 95th percentile request latency, in seconds.
    LatencyP95,
    /// Requests per second.
    RequestRate,
    /// The share of failed upstream attempts, from 0 to 1.
    UpstreamErrorRatio,
    /// The gateway's open client connections.
    OpenConnections,
}

impl MeasureName {
    fn api(self) -> &'static str {
        match self {
            Self::ServerErrorRatio => "server_error_ratio",
            Self::LatencyP95 => "latency_p95",
            Self::RequestRate => "request_rate",
            Self::UpstreamErrorRatio => "upstream_error_ratio",
            Self::OpenConnections => "open_connections",
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum SeverityName {
    Warning,
    Critical,
}

#[derive(Args)]
pub(crate) struct RuleSettings {
    /// What the rule reads, over the last five minutes.
    #[arg(long, value_enum)]
    measure: MeasureName,
    /// Fire when the measure is above this.
    #[arg(long, conflicts_with = "below", required_unless_present = "below")]
    above: Option<f64>,
    /// Fire when the measure is below this.
    #[arg(long)]
    below: Option<f64>,
    /// How long the condition holds before the rule fires, such as `5m`; at
    /// once by default.
    #[arg(long, value_parser = humantime::parse_duration)]
    pending: Option<Duration>,
    /// What people call the rule; its ID by default.
    #[arg(long)]
    name: Option<String>,
    #[arg(long)]
    description: Option<String>,
    /// Request measures: only this site.
    #[arg(long)]
    site: Option<String>,
    /// Request measures: only this route of `--site`.
    #[arg(long, requires = "site")]
    route: Option<String>,
    /// The upstream measure: only this upstream.
    #[arg(long)]
    upstream: Option<String>,
    #[arg(long, value_enum, default_value = "warning")]
    severity: SeverityName,
    /// A channel to notify; repeat for more.
    #[arg(long = "channel")]
    channels: Vec<String>,
    /// Keep the rule without evaluating it.
    #[arg(long)]
    disabled: bool,
}

impl RuleSettings {
    fn body(self, id: &str) -> Value {
        let (comparison, threshold) = match (self.above, self.below) {
            (Some(above), _) => ("above", above),
            (None, below) => ("below", below.unwrap_or_default()),
        };
        json!({
            "name": self.name.unwrap_or_else(|| id.to_owned()),
            "description": self.description.unwrap_or_default(),
            "measure": self.measure.api(),
            "comparison": comparison,
            "threshold": threshold,
            "pending_seconds": self.pending.unwrap_or_default().as_secs(),
            "site": self.site,
            "route": self.route,
            "upstream": self.upstream,
            "severity": match self.severity {
                SeverityName::Warning => "warning",
                SeverityName::Critical => "critical",
            },
            "enabled": !self.disabled,
            "channels": self.channels,
        })
    }
}

#[derive(Subcommand)]
pub(crate) enum AlertCommand {
    /// Rules that fire on the gateway's metrics.
    #[command(subcommand)]
    Rule(RuleCommand),
    /// Webhooks alert rules notify.
    #[command(subcommand)]
    Channel(ChannelCommand),
    /// Notifications sent, newest first.
    Notifications {
        /// Only one rule's notifications.
        #[arg(long)]
        rule: Option<String>,
        /// Only one channel's notifications.
        #[arg(long)]
        channel: Option<String>,
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u32).range(1..=200))]
        limit: u32,
    },
}

#[derive(Subcommand)]
pub(crate) enum RuleCommand {
    /// Every rule with where it stands.
    List,
    /// Creates a rule, or replaces it with these settings.
    Set {
        id: String,
        #[command(flatten)]
        settings: RuleSettings,
    },
    /// Deletes a rule; an alert it has firing resolves first.
    Delete { id: String },
}

#[derive(Subcommand)]
pub(crate) enum ChannelCommand {
    /// Every channel, with where it sends as its origin.
    List,
    /// Creates a webhook channel and prints its signing secret once, or a
    /// channel that notifies through a plugin with `--plugin`.
    Create {
        id: String,
        /// A file holding the webhook URL, or `-` for standard input; a URL
        /// can authorize whoever holds it, so it is never an argument.
        #[arg(long, required_unless_present = "plugin", conflicts_with = "plugin")]
        url_file: Option<PathBuf>,
        /// The plugin that delivers notifications; it needs the
        /// `notifications` grant.
        #[arg(long)]
        plugin: Option<String>,
        /// Where the plugin delivers, in its own terms, such as a chat room.
        #[arg(long, requires = "plugin")]
        plugin_channel: Option<String>,
    },
    /// Replaces a channel's signing secret, and its URL with `--url-file`,
    /// and prints the new secret once.
    Rotate {
        id: String,
        #[arg(long)]
        url_file: Option<PathBuf>,
    },
    /// Deletes a channel no rule names.
    Delete { id: String },
    /// Sends a test notification and reports how the receiver answered.
    Test { id: String },
}

fn condition(rule: &Value) -> String {
    let spec = &rule["spec"];
    let comparison = match spec["comparison"].as_str() {
        Some("below") => "<",
        _ => ">",
    };
    format!(
        "{} {comparison} {}",
        text(&spec["measure"]),
        text(&spec["threshold"])
    )
}

fn scope(rule: &Value) -> String {
    let spec = &rule["spec"];
    match (
        spec["site"].as_str(),
        spec["route"].as_str(),
        spec["upstream"].as_str(),
    ) {
        (Some(site), Some(route), _) => format!("{site}/{route}"),
        (Some(site), None, _) => site.to_owned(),
        (_, _, Some(upstream)) => format!("upstream {upstream}"),
        _ => "all".into(),
    }
}

const RULE_COLUMNS: &[Column] = &[
    ("ID", |rule| text(&rule["id"])),
    ("STATE", |rule| match rule["spec"]["enabled"].as_bool() {
        Some(false) => "disabled".into(),
        _ => text(&rule["state"]),
    }),
    ("CONDITION", condition),
    ("SCOPE", scope),
    ("VALUE", |rule| text(&rule["value"])),
    ("SINCE", |rule| text(&rule["since"])),
    ("CHANNELS", |rule| text(&rule["spec"]["channels"])),
];

const CHANNEL_COLUMNS: &[Column] = &[
    ("ID", |channel| text(&channel["id"])),
    ("KIND", |channel| text(&channel["kind"])),
    ("TARGET", |channel| text(&channel["target"])),
];

const NOTIFICATION_COLUMNS: &[Column] = &[
    ("CREATED", |notification| text(&notification["created_at"])),
    ("RULE", |notification| text(&notification["rule"])),
    ("CHANNEL", |notification| text(&notification["channel"])),
    ("KIND", |notification| text(&notification["kind"])),
    ("STATE", |notification| text(&notification["state"])),
    ("ATTEMPTS", |notification| text(&notification["attempts"])),
    ("FAILURE", |notification| {
        text(&notification["last_failure"])
    }),
];

/// A webhook URL from a file, or from standard input for `-`.
fn url(path: &Path) -> Result<String> {
    let url = if path == Path::new("-") {
        let mut url = String::new();
        std::io::stdin()
            .read_to_string(&mut url)
            .map_err(|error| CliError::Usage(format!("cannot read standard input: {error}")))?;
        url
    } else {
        read_file(path)?
    };
    let url = url.trim();
    if url.is_empty() {
        return Err(CliError::Usage("the webhook URL is empty".into()));
    }
    Ok(url.to_owned())
}

/// The ETag of item `id` of a list, for changing it; `None` when there is
/// no such item.
async fn etag(api: &Api, list: &str, id: &str) -> Result<Option<String>> {
    let items = api.get(list, &[]).await?.body;
    Ok(items
        .as_array()
        .into_iter()
        .flatten()
        .find(|item| item["id"] == id)
        .and_then(|item| item["etag"].as_str())
        .map(str::to_owned))
}

async fn existing(api: &Api, list: &str, id: &str, what: &str) -> Result<String> {
    etag(api, list, id)
        .await?
        .ok_or_else(|| CliError::Usage(format!("there is no {what} {id}")))
}

fn secret(output: &Output, message: &str, reply: &Value) {
    match output.format {
        Format::Json => output.json(reply),
        Format::Table if !output.quiet => {
            println!("{message}");
            println!("Signing secret, shown only now: {}", text(&reply["secret"]));
        }
        Format::Table => {}
    }
}

pub async fn run(api: &Api, output: &Output, command: AlertCommand) -> Result<()> {
    match command {
        AlertCommand::Rule(RuleCommand::List) => {
            let rules = api.get(RULES, &[]).await?.body;
            output.list(&rules, RULE_COLUMNS);
        }
        AlertCommand::Rule(RuleCommand::Set { id, settings }) => {
            let current = etag(api, RULES, &id).await?;
            let path = format!("{RULES}/{id}");
            let rule = api
                .change(
                    Method::PUT,
                    &path,
                    Some(&settings.body(&id)),
                    current.as_deref(),
                )
                .await?
                .body;
            let verb = if current.is_some() {
                "Replaced"
            } else {
                "Created"
            };
            output.done(
                &format!("{verb} alert rule {id}: {}", condition(&rule)),
                &rule,
            );
        }
        AlertCommand::Rule(RuleCommand::Delete { id }) => {
            let current = existing(api, RULES, &id, "alert rule").await?;
            api.change(
                Method::DELETE,
                &format!("{RULES}/{id}"),
                None,
                Some(&current),
            )
            .await?;
            output.done(&format!("Deleted alert rule {id}"), &json!({"id": id}));
        }
        AlertCommand::Channel(ChannelCommand::List) => {
            let channels = api.get(CHANNELS, &[]).await?.body;
            output.list(&channels, CHANNEL_COLUMNS);
        }
        AlertCommand::Channel(ChannelCommand::Create {
            id,
            url_file,
            plugin,
            plugin_channel,
        }) => {
            let body = match (url_file, plugin) {
                (Some(path), _) => json!({"id": id, "kind": "webhook", "url": url(&path)?}),
                (None, plugin) => json!({
                    "id": id,
                    "kind": "plugin",
                    "plugin": plugin,
                    "plugin_channel": plugin_channel,
                }),
            };
            let reply = api
                .change(Method::POST, CHANNELS, Some(&body), None)
                .await?
                .body;
            if body["kind"] == "plugin" {
                let message = format!(
                    "Created channel {id}, notifying through {}",
                    text(&reply["channel"]["target"])
                );
                output.done(&message, &reply);
            } else {
                let message = format!(
                    "Created channel {id}, posting to {}",
                    text(&reply["channel"]["target"])
                );
                secret(output, &message, &reply);
            }
        }
        AlertCommand::Channel(ChannelCommand::Rotate { id, url_file }) => {
            let current = existing(api, CHANNELS, &id, "alert channel").await?;
            let body = match url_file {
                Some(path) => json!({"url": url(&path)?}),
                None => json!({}),
            };
            let reply = api
                .change(
                    Method::POST,
                    &format!("{CHANNELS}/{id}/rotate"),
                    Some(&body),
                    Some(&current),
                )
                .await?
                .body;
            let message = format!(
                "Rotated channel {id}, posting to {}",
                text(&reply["channel"]["target"])
            );
            secret(output, &message, &reply);
        }
        AlertCommand::Channel(ChannelCommand::Delete { id }) => {
            let current = existing(api, CHANNELS, &id, "alert channel").await?;
            api.change(
                Method::DELETE,
                &format!("{CHANNELS}/{id}"),
                None,
                Some(&current),
            )
            .await?;
            output.done(&format!("Deleted channel {id}"), &json!({"id": id}));
        }
        AlertCommand::Channel(ChannelCommand::Test { id }) => {
            let tested = api
                .change(Method::POST, &format!("{CHANNELS}/{id}/test"), None, None)
                .await?
                .body;
            if tested["delivered"] != true {
                return Err(CliError::Failed(format!(
                    "channel {id} did not take the test notification: {}",
                    text(&tested["failure"])
                )));
            }
            output.done(
                &format!(
                    "Channel {id} took the test notification ({})",
                    text(&tested["status"])
                ),
                &tested,
            );
        }
        AlertCommand::Notifications {
            rule,
            channel,
            limit,
        } => {
            let mut query = vec![("limit", limit.to_string())];
            query.extend(rule.map(|rule| ("rule", rule)));
            query.extend(channel.map(|channel| ("channel", channel)));
            let notifications = api.get("/api/v1/alert-notifications", &query).await?.body;
            output.list(&notifications, NOTIFICATION_COLUMNS);
        }
    }
    Ok(())
}
