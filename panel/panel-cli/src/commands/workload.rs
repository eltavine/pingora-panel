//! Workload identities: the trusts that let programs act as service
//! accounts with their own short-lived tokens.

use crate::{
    client::{Api, CliError, Result},
    commands::identity::account_id,
    output::{text, Column, Output},
};
use clap::{Args, Subcommand};
use reqwest::Method;
use serde_json::{json, Map, Value};
use std::path::PathBuf;

fn claim(value: &str) -> std::result::Result<(String, String), String> {
    match value.split_once('=') {
        Some((name, expected)) if !name.is_empty() && !expected.is_empty() => {
            Ok((name.to_owned(), expected.to_owned()))
        }
        _ => Err("expected NAME=VALUE".into()),
    }
}

#[derive(Args)]
pub(crate) struct SetWorkload {
    id: String,
    /// The service account the workload acts as, by username or ID.
    #[arg(long)]
    account: String,
    /// The issuer's HTTPS URL, exactly as its tokens name it.
    #[arg(long)]
    issuer: String,
    /// The audience the token must name.
    #[arg(long)]
    audience: String,
    /// The subject exactly, or a prefix ending in `*`.
    #[arg(long)]
    subject: String,
    /// A further claim the token must carry, as NAME=VALUE.
    #[arg(long = "claim", value_parser = claim)]
    claims: Vec<(String, String)>,
    /// How long the sessions it grants last, 5 to 60 minutes.
    #[arg(long, default_value_t = 15)]
    session_minutes: u32,
    /// Keep the trust but admit nothing with it.
    #[arg(long)]
    disabled: bool,
}

#[derive(Subcommand)]
pub(crate) enum WorkloadCommand {
    /// Every workload identity.
    List,
    /// One workload identity.
    Show { id: String },
    /// Creates or replaces a workload identity for a service account.
    Set(Box<SetWorkload>),
    /// Deletes a workload identity.
    Delete { id: String },
    /// Exchanges a workload's token for a session secret, printed alone so
    /// that scripts can use it as PPANEL_TOKEN.
    Exchange {
        /// File with the token from the workload's issuer; - reads standard
        /// input.
        #[arg(long)]
        token_file: PathBuf,
    },
}

fn claims(trust: &Value) -> String {
    let claims: Vec<String> = trust["claims"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, value)| format!("{name}={}", text(value)))
        .collect();
    if claims.is_empty() {
        "-".into()
    } else {
        claims.join(", ")
    }
}

const TRUSTS: &[Column] = &[
    ("ID", |trust| text(&trust["id"])),
    ("ISSUER", |trust| text(&trust["issuer"])),
    ("SUBJECT", |trust| text(&trust["subject"])),
    ("CLAIMS", claims),
    ("MINUTES", |trust| text(&trust["session_minutes"])),
    ("STATE", |trust| {
        if trust["enabled"] == false {
            "disabled".into()
        } else {
            "enabled".into()
        }
    }),
];

const TRUST: &[Column] = &[
    ("ID", |trust| text(&trust["id"])),
    ("Account", |trust| text(&trust["account_id"])),
    ("Issuer", |trust| text(&trust["issuer"])),
    ("Audience", |trust| text(&trust["audience"])),
    ("Subject", |trust| text(&trust["subject"])),
    ("Claims", claims),
    ("Session minutes", |trust| text(&trust["session_minutes"])),
    ("Enabled", |trust| text(&trust["enabled"])),
    ("Updated", |trust| text(&trust["updated_at"])),
];

fn read_token(path: &PathBuf) -> Result<String> {
    let token = if path.as_os_str() == "-" {
        std::io::read_to_string(std::io::stdin())
            .map_err(|error| CliError::Usage(format!("cannot read standard input: {error}")))?
    } else {
        std::fs::read_to_string(path)
            .map_err(|error| CliError::Usage(format!("cannot read {}: {error}", path.display())))?
    };
    Ok(token.trim().to_owned())
}

pub(crate) async fn workload(api: &Api, output: &Output, command: WorkloadCommand) -> Result<()> {
    match command {
        WorkloadCommand::List => {
            let trusts = api.get("/api/v1/workload-identities", &[]).await?.body;
            output.list(&trusts, TRUSTS);
        }
        WorkloadCommand::Show { id } => {
            let trust = api
                .get(&format!("/api/v1/workload-identities/{id}"), &[])
                .await?
                .body;
            output.item(&trust, TRUST);
        }
        WorkloadCommand::Set(trust) => {
            let account = account_id(api, &trust.account).await?;
            let claims: Map<String, Value> = trust
                .claims
                .iter()
                .map(|(name, value)| (name.clone(), json!(value)))
                .collect();
            let saved = api
                .change(
                    Method::PUT,
                    &format!("/api/v1/workload-identities/{}", trust.id),
                    Some(&json!({
                        "account_id": account,
                        "issuer": trust.issuer,
                        "audience": trust.audience,
                        "subject": trust.subject,
                        "claims": claims,
                        "session_minutes": trust.session_minutes,
                        "enabled": !trust.disabled,
                    })),
                    None,
                )
                .await?
                .body;
            output.done(&format!("Saved the workload identity {}", trust.id), &saved);
        }
        WorkloadCommand::Delete { id } => {
            api.change(
                Method::DELETE,
                &format!("/api/v1/workload-identities/{id}"),
                None,
                None,
            )
            .await?;
            output.done(&format!("Deleted the workload identity {id}"), &Value::Null);
        }
        WorkloadCommand::Exchange { token_file } => {
            let token = read_token(&token_file)?;
            let session = api
                .change(
                    Method::POST,
                    "/api/v1/auth/workload",
                    Some(&json!({ "token": token })),
                    None,
                )
                .await?
                .body;
            output.done(&text(&session["secret"]), &session);
        }
    }
    Ok(())
}
