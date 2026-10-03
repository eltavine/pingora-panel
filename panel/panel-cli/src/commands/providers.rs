//! Identity providers people sign in with.

use crate::{
    client::{Api, CliError, Result},
    output::{text, Column, Output},
};
use clap::{Args, Subcommand};
use reqwest::Method;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

#[derive(Args)]
pub(crate) struct SetProvider {
    id: String,
    /// The name on the sign-in page.
    #[arg(long = "name")]
    display_name: String,
    /// The issuer URL, exactly as the provider's tokens name it.
    #[arg(long)]
    issuer: String,
    #[arg(long)]
    client_id: String,
    /// File with the client secret; the current one is kept without it.
    #[arg(long, conflicts_with = "public_client")]
    client_secret_file: Option<PathBuf>,
    /// The client has no secret and relies on PKCE alone.
    #[arg(long)]
    public_client: bool,
    /// A scope requested besides `openid`; `profile` and `email` without any.
    #[arg(long = "scope")]
    scopes: Vec<String>,
    /// Members of GROUP get ROLE at every sign-in, as GROUP=ROLE.
    #[arg(long = "group-role", value_parser = group_role)]
    group_roles: Vec<(String, String)>,
    /// Give people unknown to the panel an account at their first sign-in.
    #[arg(long)]
    create_accounts: bool,
    /// Keep the provider off the sign-in page and end its sessions.
    #[arg(long)]
    disabled: bool,
    /// The claim that names the account; `preferred_username` without it.
    #[arg(long)]
    username_claim: Option<String>,
    /// The claim that lists the person's groups; `groups` without it.
    #[arg(long)]
    groups_claim: Option<String>,
}

#[derive(Subcommand)]
pub(crate) enum IdentityProviderCommand {
    /// Every identity provider.
    List,
    /// One identity provider.
    Show { id: String },
    /// Creates or replaces an identity provider; an enabled one is checked
    /// with the provider first.
    Set(Box<SetProvider>),
    /// Deletes an identity provider; sessions signed in through it end and
    /// the accounts stay.
    Delete { id: String },
}

fn group_role(value: &str) -> std::result::Result<(String, String), String> {
    match value.split_once('=') {
        Some((group, role)) if !group.is_empty() && !role.is_empty() => {
            Ok((group.to_owned(), role.to_owned()))
        }
        _ => Err("expected GROUP=ROLE".into()),
    }
}

fn state(provider: &Value) -> String {
    if provider["enabled"].as_bool().unwrap_or(false) {
        "enabled".into()
    } else {
        "disabled".into()
    }
}

fn yes_no(value: &Value) -> String {
    if value.as_bool().unwrap_or(false) {
        "yes".into()
    } else {
        "no".into()
    }
}

fn group_roles(provider: &Value) -> String {
    let mappings: Vec<String> = provider["group_roles"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|mapping| format!("{}={}", text(&mapping["group"]), text(&mapping["role"])))
        .collect();
    if mappings.is_empty() {
        "-".into()
    } else {
        mappings.join(", ")
    }
}

const PROVIDERS: &[Column] = &[
    ("ID", |provider| text(&provider["id"])),
    ("NAME", |provider| text(&provider["display_name"])),
    ("ISSUER", |provider| text(&provider["issuer"])),
    ("STATE", state),
    ("NEW ACCOUNTS", |provider| {
        yes_no(&provider["create_accounts"])
    }),
    ("GROUP ROLES", group_roles),
];

const PROVIDER: &[Column] = &[
    ("ID", |provider| text(&provider["id"])),
    ("Name", |provider| text(&provider["display_name"])),
    ("Issuer", |provider| text(&provider["issuer"])),
    ("Client ID", |provider| text(&provider["client_id"])),
    ("Client secret", |provider| {
        if provider["has_client_secret"].as_bool().unwrap_or(false) {
            "set".into()
        } else {
            "none (public client)".into()
        }
    }),
    ("Scopes", |provider| text(&provider["scopes"])),
    ("Username claim", |provider| {
        text(&provider["claims"]["username"])
    }),
    ("Groups claim", |provider| {
        text(&provider["claims"]["groups"])
    }),
    ("Group roles", group_roles),
    ("New accounts", |provider| {
        yes_no(&provider["create_accounts"])
    }),
    ("State", state),
    ("Created", |provider| text(&provider["created_at"])),
    ("Updated", |provider| text(&provider["updated_at"])),
];

fn read(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .map(|secret| secret.trim().to_owned())
        .map_err(|error| CliError::Usage(format!("cannot read {}: {error}", path.display())))
}

fn body(provider: &SetProvider) -> Result<Value> {
    let mut body = Map::new();
    body.insert("display_name".into(), json!(provider.display_name));
    body.insert("issuer".into(), json!(provider.issuer));
    body.insert("client_id".into(), json!(provider.client_id));
    if provider.public_client {
        body.insert("client_secret".into(), Value::Null);
    } else if let Some(path) = &provider.client_secret_file {
        body.insert("client_secret".into(), json!(read(path)?));
    }
    if !provider.scopes.is_empty() {
        body.insert("scopes".into(), json!(provider.scopes));
    }
    if provider.username_claim.is_some() || provider.groups_claim.is_some() {
        body.insert(
            "claims".into(),
            json!({
                "username": provider.username_claim.as_deref().unwrap_or("preferred_username"),
                "display_name": "name",
                "email": "email",
                "groups": provider.groups_claim.as_deref().unwrap_or("groups"),
            }),
        );
    }
    body.insert(
        "group_roles".into(),
        provider
            .group_roles
            .iter()
            .map(|(group, role)| json!({"group": group, "role": role}))
            .collect(),
    );
    body.insert("create_accounts".into(), json!(provider.create_accounts));
    body.insert("enabled".into(), json!(!provider.disabled));
    Ok(Value::Object(body))
}

pub(crate) async fn identity_provider(
    api: &Api,
    output: &Output,
    command: IdentityProviderCommand,
) -> Result<()> {
    match command {
        IdentityProviderCommand::List => {
            let providers = api.get("/api/v1/identity-providers", &[]).await?.body;
            output.list(&providers, PROVIDERS);
        }
        IdentityProviderCommand::Show { id } => {
            let provider = api
                .get(&format!("/api/v1/identity-providers/{id}"), &[])
                .await?
                .body;
            output.item(&provider, PROVIDER);
        }
        IdentityProviderCommand::Set(provider) => {
            let saved = api
                .change(
                    Method::PUT,
                    &format!("/api/v1/identity-providers/{}", provider.id),
                    Some(&body(&provider)?),
                    None,
                )
                .await?
                .body;
            output.done(
                &format!("Saved the identity provider {}", provider.id),
                &saved,
            );
        }
        IdentityProviderCommand::Delete { id } => {
            api.change(
                Method::DELETE,
                &format!("/api/v1/identity-providers/{id}"),
                None,
                None,
            )
            .await?;
            output.done(&format!("Deleted the identity provider {id}"), &Value::Null);
        }
    }
    Ok(())
}
