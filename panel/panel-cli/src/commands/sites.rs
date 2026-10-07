use super::{error_pages, read_json, ActionFlags, ActionOptions, Switch};
use crate::{
    client::{Api, CliError, Result},
    output::{text, Column, Output},
};
use clap::{Subcommand, ValueEnum};
use reqwest::Method;
use serde_json::{json, Value};

#[derive(Subcommand)]
pub(crate) enum SiteCommand {
    /// Lists sites, newest page first when sorted descending.
    List {
        /// Matches names, domains, notes, groups and tags.
        #[arg(long = "search", short = 's', value_name = "TEXT")]
        q: Option<String>,
        #[arg(long, value_enum)]
        status: Option<Status>,
        #[arg(long, value_enum)]
        kind: Option<Kind>,
        #[arg(long)]
        domain: Option<String>,
        #[arg(long)]
        tag: Option<String>,
        #[arg(long)]
        group: Option<String>,
        #[arg(long)]
        favorite: bool,
        #[arg(long, value_enum, default_value_t = Sort::Name)]
        sort: Sort,
        #[arg(long)]
        descending: bool,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Follows cursors until every matching site is listed.
        #[arg(long)]
        all: bool,
    },
    /// Counts sites by status, type and HTTPS.
    Summary,
    /// Shows a site with its domains, routes and state.
    Show { id: String },
    /// Creates a site from flags, or from a JSON document with --file.
    Create(Box<CreateSite>),
    /// Replaces a site with a JSON document (the shape `show -o json` prints).
    Update {
        id: String,
        #[arg(long)]
        file: String,
    },
    /// Serves the site once the configuration is applied.
    Enable { id: String },
    /// Stops serving the site once the configuration is applied.
    Disable { id: String },
    /// Marks the site as a favorite.
    Favorite { id: String },
    /// Removes the site from the favorites.
    Unfavorite { id: String },
    /// Moves a site to the recycle bin, or removes it for good.
    Delete {
        id: String,
        #[arg(long)]
        permanent: bool,
    },
    /// Brings a site back from the recycle bin.
    Restore { id: String },
    /// Copies a site's settings and routes; domains stay with the original.
    Clone {
        id: String,
        #[arg(long)]
        name: String,
    },
    /// Writes sites with their upstreams and TLS profiles as JSON.
    Export {
        #[arg(long = "id")]
        ids: Vec<String>,
        /// File to write; standard output otherwise.
        #[arg(long)]
        file: Option<String>,
    },
    /// Imports an export as new sites.
    Import {
        #[arg(long)]
        file: String,
    },
    /// Applies one action to several sites, all or nothing.
    Batch {
        #[arg(value_enum)]
        action: BatchAction,
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Sets the pages answering a site's errors, in place of its current
    /// ones.
    ErrorPages {
        id: String,
        /// A page as the configuration language writes it, such as
        /// "404 file=errors/404.html" or "502 503 body=Back soon"; repeat it
        /// for more.
        #[arg(long = "page", value_name = "PAGE")]
        pages: Vec<String>,
        /// Whether upstreams' error responses with a page's status get the
        /// page too; unchanged unless given.
        #[arg(long, value_enum)]
        intercept: Option<Switch>,
        /// Removes every page.
        #[arg(long, conflicts_with_all = ["pages", "intercept"])]
        clear: bool,
    },
    /// Takes a site into maintenance or out of it; clients in the allowed
    /// networks still reach it.
    Maintenance {
        id: String,
        /// `off` keeps the settings for next time; `clear` removes them.
        #[arg(value_enum)]
        state: MaintenanceState,
        /// A network or address whose clients, after trusted proxies, still
        /// reach the site; repeat it for more. Replaces the current list.
        #[arg(long = "allow", value_name = "NETWORK")]
        allow: Vec<String>,
        /// The status everyone else gets; 503 unless set.
        #[arg(long)]
        status: Option<u16>,
        /// The body everyone else gets; the site's error page for the status
        /// otherwise.
        #[arg(long)]
        body: Option<String>,
        #[arg(long)]
        content_type: Option<String>,
        /// Seconds clients should wait, sent as Retry-After.
        #[arg(long, value_name = "SECONDS")]
        retry_after: Option<u32>,
    },
    /// Sets how a site answers /robots.txt itself, ahead of its routes.
    Robots {
        id: String,
        /// `off` leaves /robots.txt to the routes.
        #[arg(value_enum)]
        answer: RobotsAnswer,
        /// The file's text, for `custom`.
        #[arg(long, required_if_eq("answer", "custom"))]
        body: Option<String>,
    },
    /// Caches a site's proxied responses with a cache policy, or with none.
    Cache {
        id: String,
        /// A cache policy, or `off` to cache nothing; routes may name their
        /// own.
        policy: String,
    },
    /// Sets how a site answers /favicon.ico itself, ahead of its routes.
    Favicon {
        id: String,
        /// `off` leaves /favicon.ico to the routes.
        #[arg(value_enum)]
        answer: FaviconAnswer,
        /// The file below the gateway's static root, such as
        /// shop/favicon.ico, for `file`; the URL, for `redirect`.
        #[arg(required_if_eq_any([("answer", "file"), ("answer", "redirect")]))]
        target: Option<String>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum MaintenanceState {
    On,
    Off,
    Clear,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum RobotsAnswer {
    AllowAll,
    DisallowAll,
    Custom,
    Off,
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum FaviconAnswer {
    NoContent,
    File,
    Redirect,
    Off,
}

#[derive(clap::Args)]
pub(crate) struct CreateSite {
    #[arg(long, conflicts_with = "name")]
    file: Option<String>,
    #[arg(long, required_unless_present = "file")]
    name: Option<String>,
    /// Repeat for each domain; the first is the primary domain.
    #[arg(long = "domain")]
    domains: Vec<String>,
    #[command(flatten)]
    action: ActionFlags,
    #[command(flatten)]
    options: ActionOptions,
    #[arg(long = "tag")]
    tags: Vec<String>,
    #[arg(long)]
    group: Option<String>,
    #[arg(long)]
    note: Option<String>,
    /// Redirect plain HTTP to the site's HTTPS listener.
    #[arg(long)]
    https_redirect: bool,
    /// Serve the site's domains over HTTPS with this TLS profile.
    #[arg(long)]
    tls_profile: Option<String>,
    /// Send Strict-Transport-Security over HTTPS for this many seconds.
    #[arg(long, value_name = "SECONDS")]
    hsts_max_age: Option<u64>,
    /// Let the HSTS policy cover subdomains too.
    #[arg(long, requires = "hsts_max_age")]
    hsts_include_subdomains: bool,
    /// Consent to browsers' HSTS preload lists.
    #[arg(long, requires = "hsts_max_age")]
    hsts_preload: bool,
    /// Requests to the site pass this security policy first.
    #[arg(long)]
    security_policy: Option<String>,
    /// Requests and responses of the site go through this HTTP policy.
    #[arg(long)]
    http_policy: Option<String>,
    /// The site's proxied responses are cached by this cache policy.
    #[arg(long)]
    cache_policy: Option<String>,
    /// A rewrite rule every request runs before a route is chosen, as the
    /// configuration language writes it; repeat it for more, in order.
    #[arg(long, value_name = "RULE")]
    rewrite: Vec<String>,
    /// An error page as the configuration language writes it, such as
    /// "404 file=errors/404.html"; repeat it for more.
    #[arg(long = "error-page", value_name = "PAGE")]
    error_pages: Vec<String>,
    /// Create the site stopped.
    #[arg(long)]
    disabled: bool,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Status {
    Running,
    Stopped,
    Abnormal,
    Deleted,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Kind {
    ReverseProxy,
    Static,
    Redirect,
    Maintenance,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Sort {
    Name,
    CreatedAt,
    UpdatedAt,
    Status,
    Domain,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum BatchAction {
    Enable,
    Disable,
    Delete,
    Restore,
    Purge,
}

fn name<T: ValueEnum>(value: T) -> String {
    value
        .to_possible_value()
        .expect("every variant has a name")
        .get_name()
        .replace('-', "_")
}

const COLUMNS: &[Column] = &[
    ("ID", |site| text(&site["id"])),
    ("NAME", |site| text(&site["name"])),
    ("STATUS", |site| text(&site["status"])),
    ("TYPE", |site| text(&site["kind"])),
    ("HTTPS", |site| {
        if site["https"] == true {
            "yes".into()
        } else {
            "no".into()
        }
    }),
    ("DOMAINS", |site| {
        site["domains"]
            .as_array()
            .map(|domains| {
                domains
                    .iter()
                    .map(|domain| text(&domain["host"]))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_default()
    }),
    ("TAGS", |site| text(&site["tags"])),
];

const DETAIL: &[Column] = &[
    ("ID", |site| text(&site["id"])),
    ("Name", |site| text(&site["name"])),
    ("Status", |site| text(&site["status"])),
    ("Type", |site| text(&site["kind"])),
    ("Action", |site| site["action"].to_string()),
    ("Domains", |site| {
        site["domains"]
            .as_array()
            .map(|domains| {
                domains
                    .iter()
                    .map(|domain| {
                        let mut host = text(&domain["host"]);
                        if domain["primary"] == true {
                            host.push_str(" (primary)");
                        }
                        if domain["redirect"] == true {
                            host.push_str(" (redirects)");
                        }
                        if domain["enabled"] == false {
                            host.push_str(" (disabled)");
                        }
                        host
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default()
    }),
    ("Routes", |site| {
        site["routes"].as_array().map_or(0, Vec::len).to_string()
    }),
    ("HTTPS redirect", |site| text(&site["https_redirect"])),
    ("WWW redirect", |site| text(&site["www_redirect"])),
    ("Error pages", |site| pages(&site["error_pages"])),
    ("Cache policy", |site| text(&site["cache_policy_id"])),
    ("Maintenance", |site| {
        let maintenance = &site["maintenance"];
        let allowed = maintenance["allow"]
            .as_array()
            .map(|allow| allow.iter().map(text).collect::<Vec<_>>().join(", "))
            .filter(|allow| !allow.is_empty());
        match (maintenance["enabled"].as_bool(), allowed) {
            (None, _) => "off".into(),
            (Some(false), _) => "off (settings kept)".into(),
            (Some(true), None) => "on".into(),
            (Some(true), Some(allowed)) => format!("on, except for {allowed}"),
        }
    }),
    ("robots.txt", |site| answered(&site["robots"])),
    ("favicon.ico", |site| answered(&site["favicon"])),
    ("Group", |site| text(&site["group"])),
    ("Tags", |site| text(&site["tags"])),
    ("Note", |site| text(&site["note"])),
    ("Updated", |site| text(&site["updated_at"])),
];

pub async fn run(api: &Api, output: &Output, command: SiteCommand) -> Result<()> {
    match command {
        SiteCommand::List {
            q,
            status,
            kind,
            domain,
            tag,
            group,
            favorite,
            sort,
            descending,
            limit,
            all,
        } => {
            let mut query = vec![
                ("sort", name(sort)),
                ("descending", descending.to_string()),
                ("limit", limit.to_string()),
            ];
            for (key, value) in [
                ("q", q),
                ("status", status.map(name)),
                ("kind", kind.map(name)),
                ("domain", domain),
                ("tag", tag),
                ("group", group),
                ("favorite", favorite.then(|| "true".to_owned())),
            ] {
                if let Some(value) = value {
                    query.push((key, value));
                }
            }
            let mut items = Vec::new();
            loop {
                let page = api.get("/api/v1/sites", &query).await?.body;
                items.extend(page["items"].as_array().cloned().unwrap_or_default());
                match page["next_cursor"].as_str() {
                    Some(cursor) if all => {
                        query.retain(|(key, _)| *key != "cursor");
                        query.push(("cursor", cursor.to_owned()));
                    }
                    _ => break,
                }
            }
            output.list(&Value::Array(items), COLUMNS);
        }
        SiteCommand::Summary => {
            let summary = api.get("/api/v1/sites/summary", &[]).await?.body;
            output.item(
                &summary,
                &[
                    ("Total", |value| text(&value["total"])),
                    ("Running", |value| text(&value["running"])),
                    ("Stopped", |value| text(&value["stopped"])),
                    ("Abnormal", |value| text(&value["abnormal"])),
                    ("HTTPS", |value| text(&value["https"])),
                    ("Reverse proxy", |value| text(&value["reverse_proxy"])),
                    ("Static", |value| text(&value["static"])),
                    ("Redirect", |value| text(&value["redirect"])),
                    ("Maintenance", |value| text(&value["maintenance"])),
                    ("Recycle bin", |value| text(&value["deleted"])),
                ],
            );
        }
        SiteCommand::Show { id } => {
            let site = api.get(&format!("/api/v1/sites/{id}"), &[]).await?.body;
            output.item(&site, DETAIL);
        }
        SiteCommand::Create(create) => {
            let CreateSite {
                file,
                name,
                domains,
                action,
                options,
                tags,
                group,
                note,
                https_redirect,
                tls_profile,
                hsts_max_age,
                hsts_include_subdomains,
                hsts_preload,
                security_policy,
                http_policy,
                cache_policy,
                rewrite,
                error_pages: pages,
                disabled,
            } = *create;
            let body = match file {
                Some(file) => read_json(&file)?,
                None => json!({
                    "name": name,
                    "action": action.to_json(&options)?,
                    "enabled": !disabled,
                    "domains": domains.iter().enumerate().map(|(index, host)| json!({"host": host, "primary": index == 0})).collect::<Vec<_>>(),
                    "tags": tags,
                    "group": group,
                    "note": note,
                    "https_redirect": https_redirect,
                    "tls_profile_id": tls_profile,
                    "hsts": hsts_max_age.map(|max_age| json!({
                        "max_age_seconds": max_age,
                        "include_subdomains": hsts_include_subdomains,
                        "preload": hsts_preload,
                    })),
                    "security_policy_id": security_policy,
                    "http_policy_id": http_policy,
                    "cache_policy_id": cache_policy,
                    "rewrites": super::rewrite_rules(&rewrite)?,
                    "error_pages": {"pages": error_pages(&pages)?},
                }),
            };
            let site = api
                .change(Method::POST, "/api/v1/sites", Some(&body), None)
                .await?
                .body;
            output.done(
                &format!(
                    "Created site {} ({})",
                    text(&site["name"]),
                    text(&site["id"])
                ),
                &site,
            );
        }
        SiteCommand::Update { id, file } => {
            let body = read_json(&file)?;
            let current = api.get(&format!("/api/v1/sites/{id}"), &[]).await?;
            let site = api
                .change(
                    Method::PUT,
                    &format!("/api/v1/sites/{id}"),
                    Some(&input(body)),
                    current.etag.as_deref(),
                )
                .await?
                .body;
            output.done(&format!("Updated site {}", text(&site["name"])), &site);
        }
        SiteCommand::Enable { id } => action(api, output, &id, "enable", "Started").await?,
        SiteCommand::Disable { id } => action(api, output, &id, "disable", "Stopped").await?,
        SiteCommand::Favorite { id } => action(api, output, &id, "favorite", "Pinned").await?,
        SiteCommand::Unfavorite { id } => {
            action(api, output, &id, "unfavorite", "Unpinned").await?
        }
        SiteCommand::Restore { id } => action(api, output, &id, "restore", "Restored").await?,
        SiteCommand::Delete { id, permanent } => {
            let current = api.get(&format!("/api/v1/sites/{id}"), &[]).await?;
            let path = if permanent {
                format!("/api/v1/sites/{id}?permanent=true")
            } else {
                format!("/api/v1/sites/{id}")
            };
            let reply = api
                .change(Method::DELETE, &path, None, current.etag.as_deref())
                .await?;
            let verb = if permanent {
                "Removed"
            } else {
                "Moved to the recycle bin:"
            };
            output.done(
                &format!("{verb} {}", text(&current.body["name"])),
                &reply.body,
            );
        }
        SiteCommand::Clone { id, name } => {
            let site = api
                .change(
                    Method::POST,
                    &format!("/api/v1/sites/{id}/clone"),
                    Some(&json!({ "name": name })),
                    None,
                )
                .await?
                .body;
            output.done(
                &format!(
                    "Created site {} ({})",
                    text(&site["name"]),
                    text(&site["id"])
                ),
                &site,
            );
        }
        SiteCommand::Export { ids, file } => {
            let query: Vec<(&str, String)> = if ids.is_empty() {
                Vec::new()
            } else {
                vec![("ids", ids.join(","))]
            };
            let bundle = api.get("/api/v1/sites/export", &query).await?.body;
            let text = serde_json::to_string_pretty(&bundle).expect("JSON values serialize");
            match file {
                Some(file) => {
                    std::fs::write(&file, text).map_err(|error| {
                        CliError::Usage(format!("cannot write {file}: {error}"))
                    })?;
                    output.done(
                        &format!(
                            "Exported {} sites to {file}",
                            bundle["sites"].as_array().map_or(0, Vec::len)
                        ),
                        &json!({ "file": file }),
                    );
                }
                None => println!("{text}"),
            }
        }
        SiteCommand::Import { file } => {
            let bundle = read_json(&file)?;
            let reply = api
                .change(Method::POST, "/api/v1/sites/import", Some(&bundle), None)
                .await?
                .body;
            output.done(
                &format!(
                    "Imported {} sites",
                    reply["created"].as_array().map_or(0, Vec::len)
                ),
                &reply,
            );
        }
        SiteCommand::Batch { action, ids } => {
            let reply = api
                .change(
                    Method::POST,
                    "/api/v1/sites/batch",
                    Some(&json!({ "action": name(action), "ids": ids })),
                    None,
                )
                .await?
                .body;
            output.list(&reply, COLUMNS);
        }
        SiteCommand::ErrorPages {
            id,
            pages,
            intercept,
            clear,
        } => {
            if !clear && pages.is_empty() && intercept.is_none() {
                return Err(CliError::Usage(
                    "give pages with --page, --intercept on|off, or --clear".into(),
                ));
            }
            let pages = error_pages(&pages)?;
            edit(api, output, &id, |site| {
                site["error_pages"] = if clear {
                    json!({})
                } else {
                    let current = &site["error_pages"];
                    json!({
                        "pages": if pages.is_empty() {
                            current.get("pages").cloned().unwrap_or_else(|| json!([]))
                        } else {
                            json!(pages)
                        },
                        "intercept": intercept.map_or(current["intercept"] == true, Switch::is_on),
                    })
                };
                Ok(if clear {
                    "Removed the error pages of"
                } else {
                    "Set the error pages of"
                })
            })
            .await?;
        }
        SiteCommand::Maintenance {
            id,
            state,
            allow,
            status,
            body,
            content_type,
            retry_after,
        } => {
            edit(api, output, &id, |site| {
                if state == MaintenanceState::Clear {
                    site["maintenance"] = Value::Null;
                    return Ok("Removed the maintenance settings of");
                }
                let mut maintenance = site
                    .get("maintenance")
                    .filter(|maintenance| maintenance.is_object())
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                maintenance["enabled"] = json!(state == MaintenanceState::On);
                if !allow.is_empty() {
                    maintenance["allow"] = json!(allow);
                }
                for (field, value) in [
                    ("status", status.map(|status| json!(status))),
                    ("body", body.map(Value::from)),
                    ("content_type", content_type.map(Value::from)),
                    (
                        "retry_after_seconds",
                        retry_after.map(|seconds| json!(seconds)),
                    ),
                ] {
                    if let Some(value) = value {
                        maintenance[field] = value;
                    }
                }
                site["maintenance"] = maintenance;
                Ok(if state == MaintenanceState::On {
                    "Took into maintenance"
                } else {
                    "Took out of maintenance"
                })
            })
            .await?;
        }
        SiteCommand::Robots { id, answer, body } => {
            edit(api, output, &id, |site| {
                site["robots"] = match answer {
                    RobotsAnswer::AllowAll => json!({"kind": "allow_all"}),
                    RobotsAnswer::DisallowAll => json!({"kind": "disallow_all"}),
                    RobotsAnswer::Custom => json!({"kind": "custom", "body": body}),
                    RobotsAnswer::Off => Value::Null,
                };
                Ok("Set the robots.txt of")
            })
            .await?;
        }
        SiteCommand::Cache { id, policy } => {
            edit(api, output, &id, |site| {
                site["cache_policy_id"] = if policy == "off" {
                    Value::Null
                } else {
                    json!(policy)
                };
                Ok("Set the cache policy of")
            })
            .await?;
        }
        SiteCommand::Favicon { id, answer, target } => {
            edit(api, output, &id, |site| {
                site["favicon"] = match answer {
                    FaviconAnswer::NoContent => json!({"kind": "no_content"}),
                    FaviconAnswer::File => json!({"kind": "file", "path": target}),
                    FaviconAnswer::Redirect => json!({"kind": "redirect", "location": target}),
                    FaviconAnswer::Off => Value::Null,
                };
                Ok("Set the favicon of")
            })
            .await?;
        }
    }
    Ok(())
}

/// Reads a site, changes what `change` changes and writes it back unless
/// it changed meanwhile; `change` says what it did.
async fn edit(
    api: &Api,
    output: &Output,
    id: &str,
    change: impl FnOnce(&mut Value) -> Result<&'static str>,
) -> Result<()> {
    let current = api.get(&format!("/api/v1/sites/{id}"), &[]).await?;
    let mut body = input(current.body);
    let done = change(&mut body)?;
    let site = api
        .change(
            Method::PUT,
            &format!("/api/v1/sites/{id}"),
            Some(&body),
            current.etag.as_deref(),
        )
        .await?
        .body;
    output.done(&format!("{done} {}", text(&site["name"])), &site);
    Ok(())
}

/// The statuses error pages answer, and whether upstreams' errors get them.
fn pages(pages: &Value) -> String {
    let statuses: Vec<String> = pages["pages"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|page| page["statuses"].as_array().cloned().unwrap_or_default())
        .map(|status| text(&status))
        .collect();
    match (statuses.is_empty(), pages["intercept"] == true) {
        (true, _) => "none".into(),
        (false, false) => statuses.join(", "),
        (false, true) => format!("{}, upstreams' too", statuses.join(", ")),
    }
}

/// How a site answers a file of its own, or that its routes do.
fn answered(answer: &Value) -> String {
    let kind = text(&answer["kind"]).replace('_', " ");
    match answer["kind"].as_str() {
        None => "by the routes".into(),
        Some("custom") => "its own text".into(),
        Some("file") => format!("file {}", text(&answer["path"])),
        Some("redirect") => format!("redirect to {}", text(&answer["location"])),
        Some(_) => kind,
    }
}

async fn action(api: &Api, output: &Output, id: &str, verb: &str, done: &str) -> Result<()> {
    let site = api
        .change(
            Method::POST,
            &format!("/api/v1/sites/{id}/{verb}"),
            None,
            None,
        )
        .await?
        .body;
    output.done(&format!("{done} {}", text(&site["name"])), &site);
    Ok(())
}

/// A site representation reduced to the fields clients may write, so the
/// output of `show -o json` can be edited and sent back. Scripts and route
/// names it leaves out are kept by the service.
pub fn input(mut site: Value) -> Value {
    if let Some(object) = site.as_object_mut() {
        for field in [
            "id",
            "kind",
            "status",
            "https",
            "etag",
            "unicode_hosts",
            "created_at",
            "updated_at",
            "deleted_at",
            "lua",
        ] {
            object.remove(field);
        }
        for route in object
            .get_mut("routes")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
            .filter_map(Value::as_object_mut)
        {
            route.remove("lua");
            route.remove("named");
        }
    }
    site
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shown_sites_become_writable_inputs() {
        let site = json!({"id": "x", "name": "shop", "status": "running", "etag": "\"a\"", "action": {"type": "respond"},
                          "lua": {"access": {"inline": "return"}},
                          "routes": [{"id": "r", "named": "fallback", "lua": {}, "priority": 1}]});
        assert_eq!(
            input(site),
            json!({"name": "shop", "action": {"type": "respond"}, "routes": [{"id": "r", "priority": 1}]})
        );
        assert_eq!(pages(&json!({})), "none");
        assert_eq!(
            pages(
                &json!({"pages": [{"statuses": [404]}, {"statuses": [502, 503]}], "intercept": true})
            ),
            "404, 502, 503, upstreams' too"
        );
        assert_eq!(answered(&Value::Null), "by the routes");
        assert_eq!(answered(&json!({"kind": "disallow_all"})), "disallow all");
        assert_eq!(name(Sort::CreatedAt), "created_at");
        assert_eq!(name(Kind::ReverseProxy), "reverse_proxy");
    }
}
