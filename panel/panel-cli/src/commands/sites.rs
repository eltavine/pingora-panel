use super::{read_json, ActionFlags, ActionOptions};
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
        #[arg(long)]
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
    Show {
        id: String,
    },
    /// Creates a site from flags, or from a JSON document with --file.
    Create {
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
        /// Create the site stopped.
        #[arg(long)]
        disabled: bool,
    },
    /// Replaces a site with a JSON document (the shape `show -o json` prints).
    Update {
        id: String,
        #[arg(long)]
        file: String,
    },
    Enable {
        id: String,
    },
    Disable {
        id: String,
    },
    Favorite {
        id: String,
    },
    Unfavorite {
        id: String,
    },
    /// Moves a site to the recycle bin, or removes it for good.
    Delete {
        id: String,
        #[arg(long)]
        permanent: bool,
    },
    Restore {
        id: String,
    },
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
        SiteCommand::Create {
            file,
            name,
            domains,
            action,
            options,
            tags,
            group,
            note,
            https_redirect,
            disabled,
        } => {
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
    }
    Ok(())
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
/// output of `show -o json` can be edited and sent back.
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
        ] {
            object.remove(field);
        }
    }
    site
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shown_sites_become_writable_inputs() {
        let site = json!({"id": "x", "name": "shop", "status": "running", "etag": "\"a\"", "action": {"type": "respond"}});
        assert_eq!(
            input(site),
            json!({"name": "shop", "action": {"type": "respond"}})
        );
        assert_eq!(name(Sort::CreatedAt), "created_at");
        assert_eq!(name(Kind::ReverseProxy), "reverse_proxy");
    }
}
