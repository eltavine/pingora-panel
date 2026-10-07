use super::{read_json, rewrite_rules, route_match, ActionFlags, ActionOptions};
use crate::{
    client::{Api, CliError, Result},
    output::{text, Column, Format, Output},
};
use clap::Subcommand;
use reqwest::Method;
use serde_json::{json, Value};

#[derive(Subcommand)]
pub(crate) enum RouteCommand {
    /// Lists a site's routes in evaluation order.
    List { site: String },
    /// Shows a route with its match and action.
    Show { id: String },
    /// Adds a route; lower priorities are evaluated first.
    Add {
        site: String,
        /// KIND:PATH with KIND exact, prefix, glob or regex.
        #[arg(long = "match", value_name = "KIND:PATH")]
        matcher: String,
        /// Restricts a prefix route to one of the site's hosts.
        #[arg(long)]
        host: Option<String>,
        #[arg(long, default_value_t = 100)]
        priority: u32,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        disabled: bool,
        /// The route's requests pass this security policy after the site's.
        #[arg(long)]
        security_policy: Option<String>,
        /// The route's requests and responses go through this HTTP policy
        /// after the site's.
        #[arg(long)]
        http_policy: Option<String>,
        #[command(flatten)]
        conditions: Box<ConditionFlags>,
        #[command(flatten)]
        action: ActionFlags,
        /// Serve requests as if they asked for this path template, or for
        /// the named route @NAME, without telling the client.
        #[arg(
            long,
            value_name = "TARGET",
            conflicts_with_all = ["proxy", "static_root", "redirect", "respond", "maintenance"]
        )]
        internal_redirect: Option<String>,
        /// A rewrite rule as the configuration language writes it, such as
        /// "strip_prefix /api"; repeat it for more, in order.
        #[arg(long, value_name = "RULE")]
        rewrite: Vec<String>,
        /// Take only requests a rewrite, an internal redirect or a script
        /// sends here; others get 404.
        #[arg(long)]
        internal: bool,
        #[command(flatten)]
        options: ActionOptions,
    },
    /// Replaces a route with a JSON document.
    Update {
        id: String,
        #[arg(long)]
        file: String,
    },
    /// Lets the route match again.
    Enable { id: String },
    /// Keeps the route but stops it matching.
    Disable { id: String },
    /// Removes the route.
    Delete { id: String },
    /// Orders all of a site's routes, first to last.
    Order {
        site: String,
        #[arg(required = true)]
        ids: Vec<String>,
    },
    /// Which route of the draft a request takes, and why each route tried
    /// before it does not.
    Test {
        /// The host the request names.
        #[arg(long)]
        host: String,
        /// The request target, such as /api/items?tag=new.
        #[arg(long, default_value = "/")]
        target: String,
        #[arg(long, default_value = "GET")]
        method: String,
        /// A header line, NAME: VALUE; repeat it for more.
        #[arg(long = "header", short = 'H', value_name = "NAME: VALUE")]
        headers: Vec<String>,
        /// The client's address, after trusted proxies.
        #[arg(long)]
        client: Option<String>,
        /// The listener the request arrives on.
        #[arg(long)]
        listener: Option<String>,
    },
}

/// Conditions every request the route takes meets.
#[derive(clap::Args, Default)]
pub(crate) struct ConditionFlags {
    /// Takes only these methods, such as GET,HEAD.
    #[arg(long, value_delimiter = ',')]
    method: Vec<String>,
    /// Takes only requests with this header: NAME=VALUE, or NAME for any
    /// value.
    #[arg(long, value_name = "NAME[=VALUE]")]
    header: Vec<String>,
    /// Takes only requests with this query parameter: NAME=VALUE, or NAME.
    #[arg(long, value_name = "NAME[=VALUE]")]
    query: Vec<String>,
    /// Takes only requests with this cookie: NAME=VALUE, or NAME.
    #[arg(long, value_name = "NAME[=VALUE]")]
    cookie: Vec<String>,
    /// Takes only clients in these networks or addresses, after trusted
    /// proxies.
    #[arg(long, value_delimiter = ',')]
    client: Vec<String>,
    /// Takes only these media types, such as application/json or text/*.
    #[arg(long, value_delimiter = ',')]
    content_type: Vec<String>,
    /// Any other condition, as its JSON document.
    #[arg(long, value_name = "JSON")]
    condition: Vec<String>,
}

impl ConditionFlags {
    fn to_json(&self) -> Result<Vec<Value>> {
        let field = |kind: &str, flag: &str| match flag.split_once('=') {
            Some((name, value)) => json!({
                "kind": kind, "name": name, "test": { "op": "equals", "value": value },
            }),
            None => json!({ "kind": kind, "name": flag, "test": { "op": "present" } }),
        };
        let mut conditions = Vec::new();
        if !self.method.is_empty() {
            conditions.push(json!({ "kind": "method", "methods": self.method }));
        }
        conditions.extend(self.header.iter().map(|flag| field("header", flag)));
        conditions.extend(self.query.iter().map(|flag| field("query", flag)));
        conditions.extend(self.cookie.iter().map(|flag| field("cookie", flag)));
        if !self.client.is_empty() {
            conditions.push(json!({ "kind": "client", "networks": self.client }));
        }
        if !self.content_type.is_empty() {
            conditions.push(json!({ "kind": "content_type", "types": self.content_type }));
        }
        for condition in &self.condition {
            conditions.push(serde_json::from_str(condition).map_err(|error| {
                CliError::Usage(format!("--condition {condition:?} is not JSON: {error}"))
            })?);
        }
        Ok(conditions)
    }
}

const TRIALS: &[Column] = &[
    ("ROUTE", |trial| {
        let name = text(&trial["name"]);
        if name.is_empty() {
            text(&trial["route_id"])
        } else {
            name
        }
    }),
    ("TAKES", |trial| {
        if trial["matched"] == true {
            "yes"
        } else {
            "no"
        }
        .to_owned()
    }),
    ("WHY NOT", |trial| text(&trial["reason"])),
];

const COLUMNS: &[Column] = &[
    ("ID", |route| text(&route["id"])),
    ("PRIORITY", |route| text(&route["priority"])),
    ("NAME", |route| text(&route["name"])),
    ("MATCH", |route| {
        format!(
            "{}:{}",
            text(&route["match"]["kind"]),
            text(&route["match"]["path"])
        )
    }),
    ("ACTION", |route| text(&route["action"]["type"])),
    ("REWRITES", |route| {
        route["rewrites"].as_array().map_or(0, Vec::len).to_string()
    }),
    ("INTERNAL", |route| {
        route["internal"].as_bool().unwrap_or_default().to_string()
    }),
    ("ENABLED", |route| text(&route["enabled"])),
];

/// The fields of `route` clients write, so writing it back changes nothing
/// it does not mean to.
fn input(route: &Value) -> Value {
    json!({
        "id": route["id"],
        "name": route["name"],
        "enabled": route["enabled"],
        "priority": route["priority"],
        "match": route["match"],
        "action": route["action"],
        "security_policy_id": route.get("security_policy_id"),
        "http_policy_id": route.get("http_policy_id"),
        "access_log": route.get("access_log"),
        "rewrites": route.get("rewrites"),
        "internal": route.get("internal"),
    })
}

pub async fn run(api: &Api, output: &Output, command: RouteCommand) -> Result<()> {
    match command {
        RouteCommand::List { site } => {
            output.list(
                &api.get(&format!("/api/v1/sites/{site}/routes"), &[])
                    .await?
                    .body,
                COLUMNS,
            );
        }
        RouteCommand::Show { id } => {
            output.item(
                &api.get(&format!("/api/v1/routes/{id}"), &[]).await?.body,
                COLUMNS,
            );
        }
        RouteCommand::Add {
            site,
            matcher,
            host,
            priority,
            name,
            disabled,
            security_policy,
            http_policy,
            conditions,
            action,
            internal_redirect,
            rewrite,
            internal,
            options,
        } => {
            let action = match internal_redirect {
                Some(target) => json!({"type": "internal_redirect", "target": target}),
                None if action.is_set() => action.to_json(&options)?,
                None => {
                    return Err(CliError::Usage(
                        "choose an action: --proxy, --static, --redirect, --respond, --maintenance or --internal-redirect"
                            .into(),
                    ))
                }
            };
            let mut matched = route_match(&matcher, host.as_deref())?;
            let conditions = conditions.to_json()?;
            if !conditions.is_empty() {
                matched["conditions"] = json!(conditions);
            }
            let body = json!({
                "name": name,
                "enabled": !disabled,
                "priority": priority,
                "match": matched,
                "action": action,
                "security_policy_id": security_policy,
                "http_policy_id": http_policy,
                "rewrites": rewrite_rules(&rewrite)?,
                "internal": internal,
            });
            let route = api
                .change(
                    Method::POST,
                    &format!("/api/v1/sites/{site}/routes"),
                    Some(&body),
                    None,
                )
                .await?
                .body;
            output.done(&format!("Added route {}", text(&route["id"])), &route);
        }
        RouteCommand::Update { id, file } => {
            let body = read_json(&file)?;
            let current = api.get(&format!("/api/v1/routes/{id}"), &[]).await?;
            let route = api
                .change(
                    Method::PUT,
                    &format!("/api/v1/routes/{id}"),
                    Some(&input(&body)),
                    current.etag.as_deref(),
                )
                .await?
                .body;
            output.done(&format!("Updated route {id}"), &route);
        }
        RouteCommand::Enable { id } => toggle(api, output, &id, true).await?,
        RouteCommand::Disable { id } => toggle(api, output, &id, false).await?,
        RouteCommand::Delete { id } => {
            let current = api.get(&format!("/api/v1/routes/{id}"), &[]).await?;
            let reply = api
                .change(
                    Method::DELETE,
                    &format!("/api/v1/routes/{id}"),
                    None,
                    current.etag.as_deref(),
                )
                .await?
                .body;
            output.done(&format!("Deleted route {id}"), &reply);
        }
        RouteCommand::Order { site, ids } => {
            let routes = api
                .change(
                    Method::PUT,
                    &format!("/api/v1/sites/{site}/routes/order"),
                    Some(&json!({ "order": ids })),
                    None,
                )
                .await?
                .body;
            output.list(&routes, COLUMNS);
        }
        RouteCommand::Test {
            host,
            target,
            method,
            headers,
            client,
            listener,
        } => {
            let lines = headers
                .iter()
                .map(|line| {
                    let (name, value) = line.split_once(':').ok_or_else(|| {
                        CliError::Usage(format!("{line:?} is not a header line NAME: VALUE"))
                    })?;
                    Ok(json!({ "name": name.trim(), "value": value.trim() }))
                })
                .collect::<Result<Vec<_>>>()?;
            let body = json!({
                "method": method, "host": host, "target": target, "headers": lines,
                "client": client, "listener": listener,
            });
            let result = api
                .post_read("/api/v1/config/route-test", &body)
                .await?
                .body;
            if output.format == Format::Json {
                output.json(&result);
                return Ok(());
            }
            if result["routes"]
                .as_array()
                .is_some_and(|routes| !routes.is_empty())
            {
                output.list(&result["routes"], TRIALS);
            }
            if !output.quiet {
                let place = format!("{}{}", text(&result["host"]), text(&result["path"]));
                match result["outcome"].as_str() {
                    Some("routed") => println!(
                        "{place} is taken by route {} of site {}{}",
                        text(&result["route_id"]),
                        text(&result["site_id"]),
                        if result["default_site"] == true {
                            ", the listener's default"
                        } else {
                            ""
                        }
                    ),
                    Some("no_route") => println!(
                        "site {} has no route for {place}; the gateway answers 404",
                        text(&result["site_id"])
                    ),
                    _ => println!("no site serves {place}; the gateway answers 421"),
                }
            }
        }
    }
    Ok(())
}

async fn toggle(api: &Api, output: &Output, id: &str, enabled: bool) -> Result<()> {
    let current = api.get(&format!("/api/v1/routes/{id}"), &[]).await?;
    let mut body = input(&current.body);
    body["enabled"] = json!(enabled);
    let route = api
        .change(
            Method::PUT,
            &format!("/api/v1/routes/{id}"),
            Some(&body),
            current.etag.as_deref(),
        )
        .await?
        .body;
    let state = if enabled { "Enabled" } else { "Disabled" };
    output.done(&format!("{state} route {id}"), &route);
    Ok(())
}
