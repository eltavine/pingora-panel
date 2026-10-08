//! One module per resource; each maps subcommands onto API calls.

pub mod acme;
pub mod alerts;
pub mod approvals;
pub mod audit;
pub mod backups;
pub mod cache;
pub mod certificates;
pub mod compose;
pub mod config;
pub mod containers;
pub mod domains;
pub mod engine_resources;
pub mod files;
pub mod gateway;
pub mod host;
pub mod http_policies;
pub mod identity;
pub mod images;
pub mod logs;
pub mod lua;
pub mod plugins;
pub mod providers;
pub mod revisions;
pub mod routes;
pub mod security;
pub mod sites;
pub mod system;
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
    /// List directories without an index file, for --static.
    #[arg(long, value_enum)]
    pub autoindex: Option<Listing>,
    /// EXTENSION=TYPE, ahead of the built-in media types, for --static;
    /// repeat it for more.
    #[arg(long = "media-type", value_name = "EXTENSION=TYPE")]
    pub media_types: Vec<String>,
    /// The media type of files whose extension has none, for --static.
    #[arg(long, value_name = "TYPE")]
    pub default_type: Option<String>,
    /// A Cache-Control rule as the configuration language writes it, such as
    /// "max_age=1y immutable for=css,js" or "no_cache for=html", for
    /// --static; repeat it for more, the first naming an extension applying.
    #[arg(long = "cache-control", value_name = "RULE")]
    pub cache_control: Vec<String>,
}

/// How a directory without an index file is listed.
#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub(crate) enum Listing {
    Html,
    Json,
}

impl ActionOptions {
    /// Refuses the settings of served files for an action that serves none.
    pub fn check_files(&self, serves_files: bool) -> Result<()> {
        let written = self.autoindex.is_some()
            || !self.media_types.is_empty()
            || self.default_type.is_some()
            || !self.cache_control.is_empty();
        if written && !serves_files {
            return Err(CliError::Usage(
                "--autoindex, --media-type, --default-type and --cache-control go with --static"
                    .into(),
            ));
        }
        Ok(())
    }
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
        options.check_files(self.static_root.is_some())?;
        let mut action = Map::new();
        if let Some(upstream) = &self.proxy {
            action.insert("type".into(), json!("proxy"));
            action.insert("upstream_id".into(), json!(upstream));
        } else if let Some(root) = &self.static_root {
            action.insert("type".into(), json!("static"));
            action.insert("root".into(), json!(root));
            action.insert("spa_fallback".into(), json!(options.spa));
            if let Some(listing) = options.autoindex {
                let listing = match listing {
                    Listing::Html => "html",
                    Listing::Json => "json",
                };
                action.insert("listing".into(), json!(listing));
            }
            let mut media_types = Map::new();
            for entry in &options.media_types {
                let (extension, media_type) = entry.split_once('=').ok_or_else(|| {
                    CliError::Usage(format!("--media-type {entry:?} is not EXTENSION=TYPE"))
                })?;
                media_types.insert(
                    extension.trim_start_matches('.').to_ascii_lowercase(),
                    json!(media_type),
                );
            }
            if !media_types.is_empty() {
                action.insert("media_types".into(), Value::Object(media_types));
            }
            if let Some(default_type) = &options.default_type {
                action.insert("default_type".into(), json!(default_type));
            }
            if !options.cache_control.is_empty() {
                let rules = options
                    .cache_control
                    .iter()
                    .map(|rule| cache_rule(rule))
                    .collect::<Result<Vec<_>>>()?;
                action.insert("cache".into(), json!(rules));
            }
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

/// Rewrite rules as the configuration language writes them, such as
/// `strip_prefix /api` or `rewrite ^/old/(.*)$ /new/$1 permanent`; an
/// argument with spaces is written in double quotes.
pub fn rewrite_rules(rules: &[String]) -> Result<Vec<Value>> {
    rules
        .iter()
        .map(|rule| {
            let words = words(rule)?;
            let usage = || {
                CliError::Usage(format!(
                    "{rule:?} is not strip_prefix PATH, add_prefix PATH, set_uri TEMPLATE or rewrite REGEX REPLACEMENT [last|break|redirect|permanent]"
                ))
            };
            Ok(match words.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
                ["strip_prefix", prefix] => json!({"kind": "strip_prefix", "prefix": prefix}),
                ["add_prefix", prefix] => json!({"kind": "add_prefix", "prefix": prefix}),
                ["set_uri", template] => json!({"kind": "set_uri", "template": template}),
                ["rewrite", pattern, replacement] => {
                    json!({"kind": "rewrite", "pattern": pattern, "replacement": replacement})
                }
                ["rewrite", pattern, replacement, flag @ ("last" | "break" | "redirect" | "permanent")] => {
                    json!({"kind": "rewrite", "pattern": pattern, "replacement": replacement, "flag": flag})
                }
                _ => return Err(usage()),
            })
        })
        .collect()
}

/// A Cache-Control rule as the configuration language writes it:
/// `max_age=<duration> [immutable] [for=<extension>,...]` or
/// `no_cache [for=...]`.
fn cache_rule(rule: &str) -> Result<Value> {
    let usage = || {
        CliError::Usage(format!(
            "{rule:?} is not max_age=DURATION [immutable] [for=EXTENSIONS] or no_cache [for=EXTENSIONS]"
        ))
    };
    let (mut max_age, mut immutable, mut no_cache, mut extensions) =
        (None, false, false, Vec::new());
    for word in words(rule)? {
        match word.split_once('=') {
            Some(("max_age", value)) => max_age = Some(seconds(value).ok_or_else(usage)?),
            Some(("for", value)) => {
                extensions = value
                    .split(',')
                    .map(|extension| {
                        extension
                            .trim()
                            .trim_start_matches('.')
                            .to_ascii_lowercase()
                    })
                    .filter(|extension| !extension.is_empty())
                    .collect();
            }
            None if word == "immutable" => immutable = true,
            None if word == "no_cache" => no_cache = true,
            _ => return Err(usage()),
        }
    }
    let mut rule = Map::new();
    if !extensions.is_empty() {
        rule.insert("extensions".into(), json!(extensions));
    }
    match (max_age, no_cache) {
        (Some(seconds), false) => {
            rule.insert("max_age_seconds".into(), json!(seconds));
            if immutable {
                rule.insert("immutable".into(), json!(true));
            }
        }
        (None, true) if !immutable => {}
        _ => return Err(usage()),
    }
    Ok(Value::Object(rule))
}

/// Seconds in a duration written as nginx does: a bare number of seconds,
/// or amounts of `y` (365 days), `M` (30 days), `w`, `d`, `h`, `m` and `s`.
pub(crate) fn seconds(value: &str) -> Option<u64> {
    if value.bytes().all(|byte| byte.is_ascii_digit()) {
        return value.parse().ok();
    }
    const UNITS: [(char, u64); 7] = [
        ('y', 31_536_000),
        ('M', 2_592_000),
        ('w', 604_800),
        ('d', 86_400),
        ('h', 3_600),
        ('m', 60),
        ('s', 1),
    ];
    let (mut total, mut amount, mut last) = (0u64, String::new(), usize::MAX);
    for char in value.chars() {
        if char.is_ascii_digit() {
            amount.push(char);
            continue;
        }
        let index = UNITS.iter().position(|(unit, _)| *unit == char)?;
        if amount.is_empty() || (last != usize::MAX && index <= last) {
            return None;
        }
        total = total.checked_add(amount.parse::<u64>().ok()?.checked_mul(UNITS[index].1)?)?;
        amount.clear();
        last = index;
    }
    amount.is_empty().then_some(total)
}

/// On or off.
#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
pub(crate) enum Switch {
    On,
    Off,
}

impl Switch {
    pub fn is_on(self) -> bool {
        self == Self::On
    }
}

/// Error pages as the configuration language writes them, such as
/// `404 file=errors/404.html`, `502 503 body="<h1>Back soon</h1>"` or, as
/// nginx writes them, `404 =301 https://example.com/`.
pub fn error_pages(pages: &[String]) -> Result<Vec<Value>> {
    pages.iter().map(|page| error_page(page)).collect()
}

fn error_page(page: &str) -> Result<Value> {
    let usage = || {
        CliError::Usage(format!(
            "{page:?} is not STATUS... body=TEXT [type=TYPE], file=PATH or redirect=URL, with [status=STATUS]"
        ))
    };
    let words = words(page)?;
    let mut statuses = Vec::new();
    let mut rest = words.iter().map(String::as_str).peekable();
    while let Some(status) = rest.peek().and_then(|word| word.parse::<u16>().ok()) {
        statuses.push(status);
        rest.next();
    }
    let (mut response, mut status, mut content_type) = (None, None, None);
    for word in rest {
        let named = word.split_once('=').filter(|(key, _)| {
            matches!(*key, "" | "status" | "type" | "body" | "file" | "redirect")
        });
        let answer = match named {
            Some(("" | "status", code)) => {
                status = Some(code.parse::<u16>().map_err(|_| usage())?);
                continue;
            }
            Some(("type", value)) => {
                content_type = Some(value);
                continue;
            }
            Some(("body", body)) => json!({"kind": "body", "body": body}),
            Some(("file", path)) => json!({"kind": "file", "path": path}),
            Some((_, location)) => json!({"kind": "redirect", "location": location}),
            None if word.starts_with("http://") || word.starts_with("https://") => {
                json!({"kind": "redirect", "location": word})
            }
            None if word.starts_with('/') => json!({"kind": "file", "path": &word[1..]}),
            None => return Err(usage()),
        };
        if response.replace(answer).is_some() {
            return Err(usage());
        }
    }
    let mut response = response
        .filter(|_| !statuses.is_empty())
        .ok_or_else(usage)?;
    if let Some(content_type) = content_type {
        if response["kind"] != "body" {
            return Err(CliError::Usage(format!(
                "{page:?} gives type= to a page without body="
            )));
        }
        response["content_type"] = json!(content_type);
    }
    Ok(if response["kind"] == "redirect" {
        response["status"] = json!(status.unwrap_or(302));
        json!({"statuses": statuses, "response": response})
    } else {
        json!({"statuses": statuses, "response": response, "status": status})
    })
}

/// `text` split at spaces outside double quotes, `\"` and `\\` escaping
/// inside them.
fn words(text: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut started = false;
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            '\\' if quoted => match chars.next() {
                Some(next @ ('"' | '\\')) => word.push(next),
                Some(next) => {
                    word.push('\\');
                    word.push(next);
                }
                None => word.push('\\'),
            },
            c if c.is_whitespace() && !quoted => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            c => {
                word.push(c);
                started = true;
            }
        }
    }
    if quoted {
        return Err(CliError::Usage(format!("{text:?} leaves a quote open")));
    }
    if started {
        words.push(word);
    }
    Ok(words)
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
    fn rewrite_rules_are_read_as_the_language_writes_them() {
        let rules = rewrite_rules(&[
            "strip_prefix /api".into(),
            "rewrite ^/u/(\\d+)$ \"/users/$1?a b\" break".into(),
            "set_uri /index.php?q=$uri".into(),
        ])
        .unwrap();
        assert_eq!(
            rules,
            [
                json!({"kind": "strip_prefix", "prefix": "/api"}),
                json!({"kind": "rewrite", "pattern": "^/u/(\\d+)$", "replacement": "/users/$1?a b", "flag": "break"}),
                json!({"kind": "set_uri", "template": "/index.php?q=$uri"}),
            ]
        );
        for wrong in [
            "drop /api",
            "rewrite ^/a",
            "rewrite ^/a /b sideways",
            "set_uri \"/a",
        ] {
            assert!(rewrite_rules(&[wrong.into()]).is_err(), "{wrong}");
        }
    }

    #[test]
    fn error_pages_are_read_as_the_language_and_nginx_write_them() {
        let pages = error_pages(&[
            "404 410 \"body=<h1>Gone</h1>\" type=text/html".into(),
            "502 503 file=errors/50x.html status=503".into(),
            "403 =301 https://example.com/?from=403".into(),
            "401 /errors/401.html".into(),
        ])
        .unwrap();
        assert_eq!(
            pages,
            [
                json!({"statuses": [404, 410], "response": {"kind": "body", "body": "<h1>Gone</h1>", "content_type": "text/html"}, "status": null}),
                json!({"statuses": [502, 503], "response": {"kind": "file", "path": "errors/50x.html"}, "status": 503}),
                json!({"statuses": [403], "response": {"kind": "redirect", "location": "https://example.com/?from=403", "status": 301}}),
                json!({"statuses": [401], "response": {"kind": "file", "path": "errors/401.html"}, "status": null}),
            ]
        );
        for wrong in [
            "file=errors/404.html",
            "404",
            "404 body=a file=b",
            "404 file=a type=text/html",
            "404 status=abc body=x",
            "404 elsewhere",
        ] {
            assert!(error_pages(&[wrong.into()]).is_err(), "{wrong}");
        }
    }

    #[test]
    fn static_settings_are_read_as_the_language_writes_them() {
        let options = ActionOptions {
            autoindex: Some(Listing::Json),
            media_types: vec![
                "WASM=application/wasm".into(),
                ".map=application/json".into(),
            ],
            default_type: Some("text/plain".into()),
            cache_control: vec![
                "max_age=1y immutable for=css,.JS".into(),
                "no_cache for=html".into(),
                "max_age=1h30m".into(),
            ],
            ..ActionOptions::default()
        };
        let files = ActionFlags {
            static_root: Some("files".into()),
            ..ActionFlags::default()
        };
        assert_eq!(
            files.to_json(&options).unwrap(),
            json!({
                "type": "static", "root": "files", "spa_fallback": false, "listing": "json",
                "media_types": {"wasm": "application/wasm", "map": "application/json"},
                "default_type": "text/plain",
                "cache": [
                    {"extensions": ["css", "js"], "max_age_seconds": 31_536_000, "immutable": true},
                    {"extensions": ["html"]},
                    {"max_age_seconds": 5400},
                ],
            })
        );
        for wrong in [
            "immutable",
            "max_age=1y no_cache",
            "no_cache immutable",
            "max_age=soon",
            "max_age=1d1d",
        ] {
            assert!(cache_rule(wrong).is_err(), "{wrong}");
        }
        assert_eq!(seconds("2w"), Some(1_209_600));
        assert_eq!(seconds("90"), Some(90));
        assert_eq!(seconds("1M"), Some(2_592_000));
        let wrong_type = ActionOptions {
            media_types: vec!["wasm".into()],
            ..ActionOptions::default()
        };
        assert!(files.to_json(&wrong_type).is_err());
    }

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
