//! One module per resource; each maps subcommands onto API calls.

pub mod acme;
pub mod alerts;
pub mod approvals;
pub mod audit;
pub mod certificates;
pub mod config;
pub mod containers;
pub mod domains;
pub mod engine_resources;
pub mod gateway;
pub mod host;
pub mod identity;
pub mod images;
pub mod logs;
pub mod providers;
pub mod revisions;
pub mod routes;
pub mod security;
pub mod sites;
pub mod traffic;
pub mod upstreams;
pub mod windows;
pub mod workload;

use crate::client::{CliError, Result};
use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{json, Map, Value};
use std::path::Path;

/// A time given in RFC 3339 or as how long ago, such as `30m` or `2d`.
pub fn time(value: &str) -> std::result::Result<String, String> {
    let time = match humantime::parse_duration(value) {
        Ok(ago) => {
            Utc::now() - chrono::Duration::from_std(ago).map_err(|error| error.to_string())?
        }
        Err(_) => DateTime::parse_from_rfc3339(value)
            .map_err(|_| format!("`{value}` is neither an RFC 3339 time nor a duration like `1h`"))?
            .to_utc(),
    };
    Ok(time.to_rfc3339_opts(SecondsFormat::AutoSi, true))
}

/// Reads a file the command line names, such as a secret or a CA bundle.
pub fn read_file(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .map_err(|error| CliError::Usage(format!("cannot read {}: {error}", path.display())))
}

/// Reads a JSON document from a file, or from standard input for `-`.
pub fn read_json(path: &str) -> Result<Value> {
    let text = if path == "-" {
        std::io::read_to_string(std::io::stdin())
            .map_err(|error| CliError::Usage(format!("cannot read standard input: {error}")))?
    } else {
        std::fs::read_to_string(path)
            .map_err(|error| CliError::Usage(format!("cannot read {path}: {error}")))?
    };
    serde_json::from_str(&text)
        .map_err(|error| CliError::Usage(format!("{path} is not JSON: {error}")))
}

/// The action a site or route performs, from mutually exclusive flags.
#[derive(clap::Args, Clone, Debug, Default)]
#[group(required = false, multiple = false)]
pub struct ActionFlags {
    /// Forward to this upstream.
    #[arg(long, value_name = "UPSTREAM_ID")]
    pub proxy: Option<String>,
    /// Serve files from this directory below the gateway's static root.
    #[arg(long = "static", value_name = "ROOT")]
    pub static_root: Option<String>,
    /// Redirect to this URL or path, keeping the request path.
    #[arg(long, value_name = "URL")]
    pub redirect: Option<String>,
    /// Answer with this status.
    #[arg(long, value_name = "STATUS")]
    pub respond: Option<u16>,
    /// Answer 503 with a maintenance message.
    #[arg(long)]
    pub maintenance: bool,
}

#[derive(clap::Args, Clone, Debug, Default)]
pub struct ActionOptions {
    /// Redirect status for --redirect.
    #[arg(long, default_value_t = 308)]
    pub status: u16,
    /// Body for --respond or --maintenance.
    #[arg(long)]
    pub body: Option<String>,
    /// Seconds clients should wait, sent as Retry-After.
    #[arg(long)]
    pub retry_after: Option<u32>,
    /// Serve the index file for unknown paths (single-page apps).
    #[arg(long)]
    pub spa: bool,
}

impl ActionFlags {
    pub fn is_set(&self) -> bool {
        self.proxy.is_some()
            || self.static_root.is_some()
            || self.redirect.is_some()
            || self.respond.is_some()
            || self.maintenance
    }

    pub fn to_json(&self, options: &ActionOptions) -> Result<Value> {
        let mut action = Map::new();
        if let Some(upstream) = &self.proxy {
            action.insert("type".into(), json!("proxy"));
            action.insert("upstream_id".into(), json!(upstream));
        } else if let Some(root) = &self.static_root {
            action.insert("type".into(), json!("static"));
            action.insert("root".into(), json!(root));
            action.insert("spa_fallback".into(), json!(options.spa));
        } else if let Some(location) = &self.redirect {
            action.insert("type".into(), json!("redirect"));
            action.insert("location".into(), json!(location));
            action.insert("status".into(), json!(options.status));
        } else if self.respond.is_some() || self.maintenance {
            action.insert("type".into(), json!("respond"));
            action.insert("status".into(), json!(self.respond.unwrap_or(503)));
            let body = options.body.clone().or_else(|| {
                self.maintenance
                    .then(|| "This site is under maintenance.".to_owned())
            });
            action.insert("body".into(), json!(body));
            action.insert("retry_after_seconds".into(), json!(options.retry_after));
        } else {
            return Err(CliError::Usage(
                "choose an action: --proxy, --static, --redirect, --respond or --maintenance"
                    .into(),
            ));
        }
        Ok(Value::Object(action))
    }
}

/// `kind:path`, as in `prefix:/api` or `regex:^/v[0-9]+/`.
pub fn route_match(value: &str, host: Option<&str>) -> Result<Value> {
    let (kind, path) = value.split_once(':').ok_or_else(|| {
        CliError::Usage(format!(
            "{value:?} is not KIND:PATH (exact, prefix, glob or regex)"
        ))
    })?;
    if !matches!(kind, "exact" | "prefix" | "glob" | "regex") {
        return Err(CliError::Usage(format!(
            "{kind:?} is not exact, prefix, glob or regex"
        )));
    }
    Ok(json!({ "kind": kind, "path": path, "host": host }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_flags_become_model_actions() {
        let options = ActionOptions {
            status: 301,
            ..ActionOptions::default()
        };
        let redirect = ActionFlags {
            redirect: Some("https://example.com".into()),
            ..ActionFlags::default()
        };
        assert_eq!(
            redirect.to_json(&options).unwrap(),
            json!({"type": "redirect", "location": "https://example.com", "status": 301})
        );
        let maintenance = ActionFlags {
            maintenance: true,
            ..ActionFlags::default()
        };
        assert_eq!(maintenance.to_json(&options).unwrap()["status"], 503);
        assert!(ActionFlags::default().to_json(&options).is_err());
        assert_eq!(route_match("prefix:/api", None).unwrap()["kind"], "prefix");
        assert!(route_match("starts:/api", None).is_err());
        assert!(route_match("/api", None).is_err());
    }

    #[test]
    fn times_are_rfc_3339_or_how_long_ago() {
        assert_eq!(
            time("2027-01-15T09:00:00+01:00").unwrap(),
            "2027-01-15T08:00:00Z"
        );
        let ago = DateTime::parse_from_rfc3339(&time("1h").unwrap()).unwrap();
        let expected = Utc::now() - chrono::Duration::hours(1);
        assert!((ago.to_utc() - expected).num_seconds().abs() < 5);
        assert!(time("yesterday").is_err());
    }
}
