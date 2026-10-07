use crate::{
    client::{Api, Result},
    commands::{gateway::existing, read_json, security::size},
    output::{bytes, percent, text, Column, Format, Output},
};
use clap::{ArgGroup, Subcommand};
use reqwest::Method;
use serde_json::{json, Map, Value};

#[derive(Subcommand)]
pub(crate) enum CachePolicyCommand {
    /// Lists cache policies with the number of sites using each.
    List,
    /// Shows a cache policy.
    Show { id: String },
    /// Creates or replaces a policy from flags, or from a JSON document with
    /// --file. Sites name it with `site cache`, routes with `route cache`.
    Set(Box<PolicyArgs>),
    /// Removes a policy no site or route uses.
    Delete { id: String },
}

#[derive(clap::Args)]
pub(crate) struct PolicyArgs {
    id: String,
    /// A JSON document in the shape `show -o json` prints.
    #[arg(long, conflicts_with_all = ["ttl", "status_ttls", "key", "vary", "ignore_origin", "bypass_cookies", "bypass_queries", "bypass_headers", "stale_while_revalidate", "stale_if_error", "max_object_size", "no_status_header", "disabled"])]
    file: Option<String>,
    /// How long responses with 200, 203, 204, 300, 301 or 308 are fresh
    /// when their origin does not say, such as 10m.
    #[arg(long, value_name = "DURATION", value_parser = duration)]
    ttl: Option<u64>,
    /// How long responses with a status are fresh, as STATUS=DURATION such
    /// as 404=1m; 0 keeps the status out. Repeatable.
    #[arg(long = "status-ttl", value_name = "STATUS=DURATION", value_parser = status_ttl)]
    status_ttls: Vec<(u16, u64)>,
    /// What tells stored responses apart, a template such as
    /// '$scheme$host$request_uri', which it is unless set.
    #[arg(long, value_name = "TEMPLATE")]
    key: Option<String>,
    /// A request field whose values keep responses apart besides those the
    /// response's Vary names; repeatable.
    #[arg(long = "vary", value_name = "FIELD")]
    vary: Vec<String>,
    /// The policy alone decides, ignoring the origin's Cache-Control and
    /// Expires.
    #[arg(long)]
    ignore_origin: bool,
    /// Requests with this cookie neither use nor fill the cache; repeatable.
    #[arg(long = "bypass-cookie", value_name = "NAME")]
    bypass_cookies: Vec<String>,
    /// Requests with this query parameter neither use nor fill the cache;
    /// repeatable.
    #[arg(long = "bypass-query", value_name = "NAME")]
    bypass_queries: Vec<String>,
    /// Requests with this header neither use nor fill the cache; repeatable.
    #[arg(long = "bypass-header", value_name = "NAME")]
    bypass_headers: Vec<String>,
    /// How long a stale response is served while it is revalidated, unless
    /// the origin says.
    #[arg(long, value_name = "DURATION", value_parser = duration)]
    stale_while_revalidate: Option<u64>,
    /// How long a stale response is served when the upstream fails, unless
    /// the origin says.
    #[arg(long, value_name = "DURATION", value_parser = duration)]
    stale_if_error: Option<u64>,
    /// The largest response stored, such as 16m; 8m unless set, at most 64m.
    #[arg(long, value_name = "SIZE", value_parser = size)]
    max_object_size: Option<u64>,
    /// Responses do not carry Cache-Status.
    #[arg(long)]
    no_status_header: bool,
    /// The policy caches nothing for those naming it.
    #[arg(long)]
    disabled: bool,
}

#[derive(Subcommand)]
pub(crate) enum CacheCommand {
    /// What the gateway's cache holds, and how its lookups for each site
    /// went since the gateway started.
    Stats,
    /// Purges the gateway's cache: everything, sites' responses, or URLs.
    /// What is purged is fetched again when next asked for.
    #[command(group(ArgGroup::new("target").required(true).args(["all", "sites", "urls"])))]
    Purge {
        #[arg(long)]
        all: bool,
        /// A site whose responses are purged; repeatable.
        #[arg(long = "site", value_name = "ID")]
        sites: Vec<String>,
        /// A URL whose stored responses are purged, such as
        /// https://shop.example/; repeatable.
        #[arg(long = "url", value_name = "URL")]
        urls: Vec<String>,
    },
    /// Shows or sets how much the cache keeps, applied with the
    /// configuration; a new size empties it.
    Store {
        /// Such as 512m or 2g, between 1m and 64g.
        #[arg(long, value_name = "SIZE", value_parser = size, conflicts_with = "default")]
        max_size: Option<u64>,
        /// Back to 256m.
        #[arg(long)]
        default: bool,
    },
}

fn duration(value: &str) -> std::result::Result<u64, String> {
    super::seconds(value)
        .ok_or_else(|| format!("{value:?} is not a duration such as 30s, 10m or 1d"))
}

/// `STATUS=DURATION`.
fn status_ttl(value: &str) -> std::result::Result<(u16, u64), String> {
    let usage = || format!("{value:?} is not STATUS=DURATION such as 404=1m");
    let (status, time) = value.split_once('=').ok_or_else(usage)?;
    let status = status
        .parse::<u16>()
        .ok()
        .filter(|status| (100..=599).contains(status))
        .ok_or_else(usage)?;
    Ok((status, duration(time)?))
}

const POLICIES: &[Column] = &[
    ("ID", |policy| text(&policy["id"])),
    ("ENABLED", |policy| (policy["enabled"] != false).to_string()),
    ("TTL", |policy| match policy["ttl_seconds"].as_u64() {
        Some(seconds) if seconds > 0 => {
            humantime::format_duration(std::time::Duration::from_secs(seconds)).to_string()
        }
        _ => "origin's".into(),
    }),
    ("BY STATUS", |policy| {
        policy["status_ttls"]
            .as_object()
            .map(|times| {
                times
                    .iter()
                    .map(|(status, seconds)| format!("{status}={}s", text(seconds)))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .filter(|times| !times.is_empty())
            .unwrap_or_else(|| "-".into())
    }),
    ("BYPASS", |policy| {
        policy["bypass"].as_array().map_or(0, Vec::len).to_string()
    }),
    ("USED BY", |policy| {
        policy["used_by"]
            .as_array()
            .map_or_else(String::new, |sites| sites.len().to_string())
    }),
];

const STATS: &[Column] = &[
    ("Size", |stats| {
        format!(
            "{} of {}",
            bytes(&stats["bytes"]),
            bytes(&stats["max_bytes"])
        )
    }),
    ("Entries", |stats| text(&stats["entries"])),
    ("Since", |stats| text(&stats["since"])),
    ("Observed", |stats| text(&stats["observed_at"])),
];

const SITES: &[Column] = &[
    ("SITE", |site| text(&site["site_id"])),
    ("HIT RATIO", |site| percent(&site["hit_ratio"])),
    ("HITS", |site| text(&site["hits"])),
    ("STALE", |site| text(&site["stale"])),
    ("MISSES", |site| text(&site["misses"])),
    ("EXPIRED", |site| text(&site["expired"])),
    ("REVALIDATED", |site| text(&site["revalidated"])),
    ("BYPASSES", |site| text(&site["bypasses"])),
    ("UNCACHEABLE", |site| text(&site["uncacheable"])),
];

/// The policy `flags` describe.
fn policy(id: &str, flags: PolicyArgs) -> Value {
    let mut policy = Map::new();
    policy.insert("id".into(), json!(id));
    let mut put = |field: &str, value: Value| {
        policy.insert(field.into(), value);
    };
    if flags.disabled {
        put("enabled", json!(false));
    }
    if let Some(seconds) = flags.ttl {
        put("ttl_seconds", json!(seconds));
    }
    if !flags.status_ttls.is_empty() {
        let times: Map<String, Value> = flags
            .status_ttls
            .iter()
            .map(|(status, seconds)| (status.to_string(), json!(seconds)))
            .collect();
        put("status_ttls", Value::Object(times));
    }
    if let Some(key) = flags.key {
        put("key", json!(key));
    }
    if !flags.vary.is_empty() {
        put("vary_headers", json!(flags.vary));
    }
    if flags.ignore_origin {
        put("honor_origin", json!(false));
    }
    let present = |kind: &str, names: &[String]| {
        names
            .iter()
            .map(|name| json!({"kind": kind, "name": name, "test": {"op": "present"}}))
            .collect::<Vec<_>>()
    };
    let bypass: Vec<Value> = [
        present("cookie", &flags.bypass_cookies),
        present("query", &flags.bypass_queries),
        present("header", &flags.bypass_headers),
    ]
    .concat();
    if !bypass.is_empty() {
        put("bypass", json!(bypass));
    }
    if let Some(seconds) = flags.stale_while_revalidate {
        put("stale_while_revalidate_seconds", json!(seconds));
    }
    if let Some(seconds) = flags.stale_if_error {
        put("stale_if_error_seconds", json!(seconds));
    }
    if let Some(bytes) = flags.max_object_size {
        put("max_object_bytes", json!(bytes));
    }
    if flags.no_status_header {
        put("status_header", json!(false));
    }
    Value::Object(policy)
}

pub async fn policies(api: &Api, output: &Output, command: CachePolicyCommand) -> Result<()> {
    match command {
        CachePolicyCommand::List => output.list(
            &api.get("/api/v1/cache-policies", &[]).await?.body,
            POLICIES,
        ),
        CachePolicyCommand::Show { id } => output.item(
            &api.get(&format!("/api/v1/cache-policies/{id}"), &[])
                .await?
                .body,
            POLICIES,
        ),
        CachePolicyCommand::Set(flags) => {
            let id = flags.id.clone();
            let path = format!("/api/v1/cache-policies/{id}");
            let etag = existing(api, &path).await?;
            let body = match &flags.file {
                Some(file) => {
                    let mut body = read_json(file)?;
                    if let Some(object) = body.as_object_mut() {
                        object.remove("used_by");
                        object.remove("etag");
                        object.insert("id".into(), json!(id));
                    }
                    body
                }
                None => policy(&id, *flags),
            };
            let saved = api
                .change(Method::PUT, &path, Some(&body), etag.as_deref())
                .await?
                .body;
            output.done(&format!("Saved cache policy {id}"), &saved);
        }
        CachePolicyCommand::Delete { id } => {
            let path = format!("/api/v1/cache-policies/{id}");
            let etag = existing(api, &path).await?;
            let reply = api
                .change(Method::DELETE, &path, None, etag.as_deref())
                .await?
                .body;
            output.done(&format!("Deleted cache policy {id}"), &reply);
        }
    }
    Ok(())
}

pub async fn run(api: &Api, output: &Output, command: CacheCommand) -> Result<()> {
    match command {
        CacheCommand::Stats => {
            let stats = api.get("/api/v1/gateway/cache", &[]).await?.body;
            if output.format == Format::Json {
                output.json(&stats);
            } else if !output.quiet {
                output.item(&stats, STATS);
                if stats["sites"]
                    .as_array()
                    .is_some_and(|sites| !sites.is_empty())
                {
                    println!();
                    output.list(&stats["sites"], SITES);
                }
            }
        }
        CacheCommand::Purge { all, sites, urls } => {
            let body = json!({"all": all, "site_ids": sites, "urls": urls});
            let purged = api
                .change(
                    Method::POST,
                    "/api/v1/gateway/cache/purge",
                    Some(&body),
                    None,
                )
                .await?
                .body;
            output.done(
                &format!("Purged {} cached keys", text(&purged["keys"])),
                &purged,
            );
        }
        CacheCommand::Store { max_size, default } => {
            let current = api.get("/api/v1/cache-settings", &[]).await?;
            if max_size.is_none() && !default {
                output.item(
                    &current.body,
                    &[("Max size", |store| match store.get("max_bytes") {
                        Some(size) => bytes(size),
                        None => "256.0 MiB (default)".into(),
                    })],
                );
                return Ok(());
            }
            let body = json!({"max_bytes": max_size});
            let store = api
                .change(
                    Method::PUT,
                    "/api/v1/cache-settings",
                    Some(&body),
                    current.etag.as_deref(),
                )
                .await?
                .body;
            output.done(
                "Set the cache store's size; it applies with the configuration",
                &store,
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_and_durations_read_like_the_configuration_language() {
        assert_eq!(status_ttl("404=1m"), Ok((404, 60)));
        assert_eq!(status_ttl("500=0"), Ok((500, 0)));
        assert!(status_ttl("4040=1m").is_err() && status_ttl("404").is_err());
        assert_eq!(duration("1h30m"), Ok(5_400));
    }
}
