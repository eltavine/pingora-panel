use super::{read_json, route_match, ActionFlags, ActionOptions};
use crate::{
    client::{Api, CliError, Result},
    output::{text, Column, Output},
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
        #[command(flatten)]
        action: ActionFlags,
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
}

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
    ("ENABLED", |route| text(&route["enabled"])),
];

fn input(route: &Value) -> Value {
    json!({
        "id": route["id"],
        "name": route["name"],
        "enabled": route["enabled"],
        "priority": route["priority"],
        "match": route["match"],
        "action": route["action"],
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
            action,
            options,
        } => {
            if !action.is_set() {
                return Err(CliError::Usage(
                    "choose an action: --proxy, --static, --redirect, --respond or --maintenance"
                        .into(),
                ));
            }
            let body = json!({
                "name": name,
                "enabled": !disabled,
                "priority": priority,
                "match": route_match(&matcher, host.as_deref())?,
                "action": action.to_json(&options)?,
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
