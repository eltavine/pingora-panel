//! ACME accounts and the automatic certificates they obtain and renew.

use super::read_file;
use crate::{
    client::{Api, CliError, Result},
    output::{text, Column, Output},
};
use clap::{Subcommand, ValueEnum};
use reqwest::Method;
use serde_json::json;
use std::path::PathBuf;

#[derive(Subcommand)]
pub(crate) enum AcmeCommand {
    /// Accounts with ACME CAs.
    Account {
        #[command(subcommand)]
        command: AccountCommand,
    },
    /// Certificates obtained from ACME CAs and kept renewed.
    Certificate {
        #[command(subcommand)]
        command: AutomaticCommand,
    },
    /// DNS servers that publish DNS-01 records.
    #[command(name = "dns-provider")]
    DnsProvider {
        #[command(subcommand)]
        command: DnsProviderCommand,
    },
}

/// Where an RFC 2136 provider sends updates.
#[derive(clap::Args)]
pub(crate) struct Rfc2136Flags {
    /// The zones' primary server as host:port.
    #[arg(long)]
    server: String,
    /// A zone the key may update, such as example.com; repeatable.
    #[arg(long = "zone", required = true)]
    zones: Vec<String>,
    /// The name of the TSIG key.
    #[arg(long)]
    key_name: String,
    #[arg(long, value_enum, default_value_t = TsigAlgorithm::HmacSha256)]
    algorithm: TsigAlgorithm,
    /// TTL of published records in seconds.
    #[arg(long)]
    ttl: Option<u32>,
    /// Seconds to wait for records to reach every authoritative server.
    #[arg(long)]
    propagation: Option<u32>,
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum TsigAlgorithm {
    #[value(name = "hmac-sha256")]
    HmacSha256,
    #[value(name = "hmac-sha512")]
    HmacSha512,
}

impl TsigAlgorithm {
    fn name(self) -> &'static str {
        match self {
            Self::HmacSha256 => "hmac-sha256",
            Self::HmacSha512 => "hmac-sha512",
        }
    }
}

impl Rfc2136Flags {
    fn settings(&self) -> serde_json::Value {
        let mut settings = json!({
            "server": self.server,
            "zones": self.zones,
            "key_name": self.key_name,
            "algorithm": self.algorithm.name(),
        });
        if let Some(ttl) = self.ttl {
            settings["ttl"] = json!(ttl);
        }
        settings
    }
}

#[derive(Subcommand)]
pub(crate) enum DnsProviderCommand {
    /// Every DNS provider.
    List,
    /// A DNS provider's settings; its secret is never shown.
    Show { id: String },
    /// Keeps an RFC 2136 primary that accepts updates signed with a TSIG key.
    Add {
        /// Lowercase letters, digits and hyphens, such as primary-ns.
        id: String,
        #[command(flatten)]
        rfc2136: Rfc2136Flags,
        /// File with the base64 TSIG secret, as in BIND key files.
        #[arg(long)]
        secret_file: PathBuf,
    },
    /// Changes a provider's settings, and its secret when a file is given.
    Update {
        id: String,
        #[command(flatten)]
        rfc2136: Rfc2136Flags,
        #[arg(long)]
        secret_file: Option<PathBuf>,
    },
    /// Forgets a provider no automatic certificate uses.
    Delete { id: String },
}

#[derive(Subcommand)]
pub(crate) enum AccountCommand {
    /// Every account.
    List,
    /// An account with its CA and contacts.
    Show { id: String },
    /// Registers an account with a CA.
    Register {
        /// Lowercase letters, digits and hyphens, such as letsencrypt.
        id: String,
        /// letsencrypt, letsencrypt-staging, zerossl, google, or the URL of
        /// any ACME directory.
        #[arg(long)]
        directory: String,
        /// An address the CA may write to; repeatable.
        #[arg(long = "email")]
        emails: Vec<String>,
        /// Accept the CA's terms of service, which registering requires.
        #[arg(long)]
        agree_tos: bool,
        /// File with PEM roots to trust for a private directory.
        #[arg(long)]
        ca_bundle: Option<PathBuf>,
        /// Key identifier of an external account binding, which some CAs
        /// require.
        #[arg(long, requires = "eab_mac_key_file")]
        eab_key_id: Option<String>,
        /// File with the base64url MAC key of the external account binding.
        #[arg(long, requires = "eab_key_id")]
        eab_mac_key_file: Option<PathBuf>,
    },
    /// Forgets an account no automatic certificate uses.
    Delete { id: String },
}

#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum Challenge {
    /// A file the gateway serves on port 80 for every name.
    #[value(name = "http-01")]
    Http01,
    /// A TXT record in every name's DNS zone; needed for wildcards.
    #[value(name = "dns-01")]
    Dns01,
}

impl Challenge {
    fn name(self) -> &'static str {
        match self {
            Self::Http01 => "http-01",
            Self::Dns01 => "dns-01",
        }
    }
}

#[derive(Subcommand)]
pub(crate) enum AutomaticCommand {
    /// Every automatic certificate and when it renews.
    List,
    /// An automatic certificate with its last failure.
    Show { id: String },
    /// Obtains a certificate and keeps it renewed; the first issuance runs
    /// in the background.
    Request {
        /// The certificate's ID in the inventory, such as example.com.
        id: String,
        /// The ACME account that orders it.
        #[arg(long)]
        account: String,
        /// A DNS name, a wildcard such as *.example.com, or an IP address;
        /// repeatable.
        #[arg(long = "name", required = true)]
        names: Vec<String>,
        #[arg(long, value_enum, default_value_t = Challenge::Http01)]
        challenge: Challenge,
        /// The DNS provider that publishes the records of dns-01.
        #[arg(long)]
        dns_provider: Option<String>,
    },
    /// Issues an automatic certificate again now.
    Renew { id: String },
    /// Stops renewing a certificate; it stays in the inventory.
    Delete { id: String },
}

/// Directories of well-known public CAs.
const DIRECTORIES: &[(&str, &str)] = &[
    (
        "letsencrypt",
        "https://acme-v02.api.letsencrypt.org/directory",
    ),
    (
        "letsencrypt-staging",
        "https://acme-staging-v02.api.letsencrypt.org/directory",
    ),
    ("zerossl", "https://acme.zerossl.com/v2/DV90"),
    ("google", "https://dv.acme-v02.api.pki.goog/directory"),
];

fn directory(value: &str) -> String {
    DIRECTORIES
        .iter()
        .find(|(name, _)| *name == value)
        .map_or_else(|| value.to_owned(), |(_, url)| (*url).to_owned())
}

const DNS_PROVIDERS: &[Column] = &[
    ("ID", |provider| text(&provider["id"])),
    ("KIND", |provider| text(&provider["kind"])),
    ("SERVER", |provider| text(&provider["rfc2136"]["server"])),
    ("ZONES", |provider| text(&provider["rfc2136"]["zones"])),
    ("KEY", |provider| text(&provider["rfc2136"]["key_name"])),
];

const DNS_PROVIDER: &[Column] = &[
    ("ID", |provider| text(&provider["id"])),
    ("KIND", |provider| text(&provider["kind"])),
    ("SERVER", |provider| text(&provider["rfc2136"]["server"])),
    ("ZONES", |provider| text(&provider["rfc2136"]["zones"])),
    ("KEY", |provider| text(&provider["rfc2136"]["key_name"])),
    ("ALGORITHM", |provider| {
        text(&provider["rfc2136"]["algorithm"])
    }),
    ("TTL", |provider| text(&provider["rfc2136"]["ttl"])),
    ("PROPAGATION", |provider| {
        text(&provider["propagation_seconds"])
    }),
    ("VERSION", |provider| text(&provider["version"])),
];

const ACCOUNTS: &[Column] = &[
    ("ID", |account| text(&account["id"])),
    ("DIRECTORY", |account| text(&account["directory"])),
    ("CONTACT", |account| text(&account["contact"])),
    ("URL", |account| text(&account["url"])),
];

const ACCOUNT: &[Column] = &[
    ("ID", |account| text(&account["id"])),
    ("DIRECTORY", |account| text(&account["directory"])),
    ("CONTACT", |account| text(&account["contact"])),
    ("EAB KEY ID", |account| {
        text(&account["external_account_key_id"])
    }),
    ("CA BUNDLE", |account| {
        text(&json!(!account["ca_bundle"].is_null()))
    }),
    ("URL", |account| text(&account["url"])),
    ("VERSION", |account| text(&account["version"])),
    ("CREATED", |account| text(&account["created_at"])),
];

const AUTOMATIC: &[Column] = &[
    ("ID", |certificate| text(&certificate["id"])),
    ("NAMES", |certificate| text(&certificate["names"])),
    ("STATE", |certificate| text(&certificate["state"])),
    ("CHALLENGE", |certificate| text(&certificate["challenge"])),
    ("ACCOUNT", |certificate| text(&certificate["account"])),
    ("RENEW AFTER", |certificate| {
        text(&certificate["renew_after"])
    }),
    ("FAILURES", |certificate| text(&certificate["failures"])),
];

const AUTOMATIC_DETAIL: &[Column] = &[
    ("ID", |certificate| text(&certificate["id"])),
    ("STATE", |certificate| text(&certificate["state"])),
    ("NAMES", |certificate| text(&certificate["names"])),
    ("ACCOUNT", |certificate| text(&certificate["account"])),
    ("CHALLENGE", |certificate| text(&certificate["challenge"])),
    ("DNS PROVIDER", |certificate| {
        text(&certificate["dns_provider"])
    }),
    ("RENEW AFTER", |certificate| {
        text(&certificate["renew_after"])
    }),
    ("WINDOW", |certificate| {
        text(&certificate["renewal_explanation_url"])
    }),
    ("FAILURES", |certificate| text(&certificate["failures"])),
    ("LAST ERROR", |certificate| {
        text(&certificate["last_error"]["message"])
    }),
    ("FAILED AT", |certificate| {
        text(&certificate["last_error"]["at"])
    }),
    ("VERSION", |certificate| text(&certificate["version"])),
];

/// The current entity tag, for deleting.
async fn etag(api: &Api, path: &str) -> Result<String> {
    api.get(path, &[])
        .await?
        .etag
        .ok_or_else(|| CliError::Usage(format!("{path} has no entity tag")))
}

async fn accounts(api: &Api, output: &Output, command: AccountCommand) -> Result<()> {
    match command {
        AccountCommand::List => {
            output.list(&api.get("/api/v1/acme-accounts", &[]).await?.body, ACCOUNTS);
        }
        AccountCommand::Show { id } => {
            let account = api
                .get(&format!("/api/v1/acme-accounts/{id}"), &[])
                .await?
                .body;
            output.item(&account, ACCOUNT);
        }
        AccountCommand::Register {
            id,
            directory: url,
            emails,
            agree_tos,
            ca_bundle,
            eab_key_id,
            eab_mac_key_file,
        } => {
            if !agree_tos {
                return Err(CliError::Usage(
                    "registering accepts the CA's terms of service; pass --agree-tos".into(),
                ));
            }
            let mut body = json!({
                "id": id,
                "directory": directory(&url),
                "contact": emails,
                "terms_of_service_agreed": true,
            });
            if let Some(path) = ca_bundle {
                body["ca_bundle"] = json!(read_file(&path)?);
            }
            if let (Some(key_id), Some(path)) = (eab_key_id, eab_mac_key_file) {
                body["external_account"] =
                    json!({ "key_id": key_id, "mac_key": read_file(&path)?.trim() });
            }
            let account = api
                .change(Method::POST, "/api/v1/acme-accounts", Some(&body), None)
                .await?
                .body;
            output.done(&format!("Registered ACME account {id}"), &account);
        }
        AccountCommand::Delete { id } => {
            let path = format!("/api/v1/acme-accounts/{id}");
            let tag = etag(api, &path).await?;
            let reply = api
                .change(Method::DELETE, &path, None, Some(&tag))
                .await?
                .body;
            output.done(&format!("Deleted ACME account {id}"), &reply);
        }
    }
    Ok(())
}

async fn automatic(api: &Api, output: &Output, command: AutomaticCommand) -> Result<()> {
    match command {
        AutomaticCommand::List => output.list(
            &api.get("/api/v1/acme-certificates", &[]).await?.body,
            AUTOMATIC,
        ),
        AutomaticCommand::Show { id } => {
            let certificate = api
                .get(&format!("/api/v1/acme-certificates/{id}"), &[])
                .await?
                .body;
            output.item(&certificate, AUTOMATIC_DETAIL);
        }
        AutomaticCommand::Request {
            id,
            account,
            names,
            challenge,
            dns_provider,
        } => {
            let mut body = json!({
                "id": id,
                "account": account,
                "names": names,
                "challenge": challenge.name(),
            });
            if let Some(provider) = dns_provider {
                body["dns_provider"] = json!(provider);
            }
            let certificate = api
                .change(Method::POST, "/api/v1/acme-certificates", Some(&body), None)
                .await?
                .body;
            output.done(
                &format!("Requested certificate {id}; it is issued in the background"),
                &certificate,
            );
        }
        AutomaticCommand::Renew { id } => {
            let certificate = api
                .change(
                    Method::POST,
                    &format!("/api/v1/acme-certificates/{id}/renewals"),
                    None,
                    None,
                )
                .await?
                .body;
            output.done(
                &format!("Renewing certificate {id} in the background"),
                &certificate,
            );
        }
        AutomaticCommand::Delete { id } => {
            let path = format!("/api/v1/acme-certificates/{id}");
            let tag = etag(api, &path).await?;
            let reply = api
                .change(Method::DELETE, &path, None, Some(&tag))
                .await?
                .body;
            output.done(
                &format!("Stopped renewing {id}; the certificate stays in the inventory"),
                &reply,
            );
        }
    }
    Ok(())
}

async fn dns_providers(api: &Api, output: &Output, command: DnsProviderCommand) -> Result<()> {
    match command {
        DnsProviderCommand::List => output.list(
            &api.get("/api/v1/dns-providers", &[]).await?.body,
            DNS_PROVIDERS,
        ),
        DnsProviderCommand::Show { id } => {
            let provider = api
                .get(&format!("/api/v1/dns-providers/{id}"), &[])
                .await?
                .body;
            output.item(&provider, DNS_PROVIDER);
        }
        DnsProviderCommand::Add {
            id,
            rfc2136,
            secret_file,
        } => {
            let mut body = json!({
                "id": id,
                "kind": "rfc2136",
                "rfc2136": rfc2136.settings(),
                "secret": read_file(&secret_file)?.trim(),
            });
            if let Some(seconds) = rfc2136.propagation {
                body["propagation_seconds"] = json!(seconds);
            }
            let provider = api
                .change(Method::POST, "/api/v1/dns-providers", Some(&body), None)
                .await?
                .body;
            output.done(&format!("Added DNS provider {id}"), &provider);
        }
        DnsProviderCommand::Update {
            id,
            rfc2136,
            secret_file,
        } => {
            let path = format!("/api/v1/dns-providers/{id}");
            let mut body = json!({ "rfc2136": rfc2136.settings() });
            if let Some(file) = secret_file {
                body["secret"] = json!(read_file(&file)?.trim());
            }
            if let Some(seconds) = rfc2136.propagation {
                body["propagation_seconds"] = json!(seconds);
            }
            let tag = etag(api, &path).await?;
            let provider = api
                .change(Method::PUT, &path, Some(&body), Some(&tag))
                .await?
                .body;
            output.done(&format!("Updated DNS provider {id}"), &provider);
        }
        DnsProviderCommand::Delete { id } => {
            let path = format!("/api/v1/dns-providers/{id}");
            let tag = etag(api, &path).await?;
            let reply = api
                .change(Method::DELETE, &path, None, Some(&tag))
                .await?
                .body;
            output.done(&format!("Deleted DNS provider {id}"), &reply);
        }
    }
    Ok(())
}

pub async fn run(api: &Api, output: &Output, command: AcmeCommand) -> Result<()> {
    match command {
        AcmeCommand::Account { command } => accounts(api, output, command).await,
        AcmeCommand::Certificate { command } => automatic(api, output, command).await,
        AcmeCommand::DnsProvider { command } => dns_providers(api, output, command).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_directories_have_names() {
        assert_eq!(
            directory("letsencrypt-staging"),
            "https://acme-staging-v02.api.letsencrypt.org/directory"
        );
        assert_eq!(
            directory("https://ca.example/acme"),
            "https://ca.example/acme"
        );
    }
}
