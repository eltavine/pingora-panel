use crate::{
    client::{Api, CliError, Result},
    output::{text, Column, Output},
};
use clap::Subcommand;
use reqwest::Method;
use serde_json::{json, Value};

#[derive(Subcommand)]
pub(crate) enum ListenerCommand {
    /// Lists listeners with their protocols and default sites.
    List,
    /// Shows a listener.
    Show { id: String },
    /// Creates or replaces a listener.
    Set {
        id: String,
        /// IP:PORT, for example 0.0.0.0:80 or [::]:443.
        #[arg(long)]
        address: String,
        /// Serve HTTPS with this profile's certificate by default.
        #[arg(long)]
        tls_profile: Option<String>,
        #[arg(long)]
        no_http1: bool,
        #[arg(long)]
        no_http2: bool,
        /// Reserved: recorded but not yet served.
        #[arg(long)]
        http3: bool,
        #[arg(long)]
        reuse_port: bool,
        /// For IPv6 addresses: accept only IPv6 (true) or both (false).
        #[arg(long)]
        ipv6_only: Option<bool>,
        /// Site serving hosts no other site claims.
        #[arg(long)]
        default_site: Option<String>,
    },
    /// Removes a listener no site uses.
    Delete { id: String },
}

#[derive(Subcommand)]
pub(crate) enum TlsProfileCommand {
    /// Lists TLS profiles.
    List,
    /// Shows a TLS profile.
    Show { id: String },
    /// Creates or replaces a profile naming files in the gateway's secret directory.
    Set {
        id: String,
        /// File name of the PEM certificate chain.
        #[arg(long)]
        certificate: String,
        /// File name of the PEM private key.
        #[arg(long)]
        key: String,
        #[arg(long, default_value = "TLSv1.2")]
        min_protocol: String,
        /// Protocol a listener using this profile offers through ALPN, h2 or
        /// http/1.1; repeatable. Without it the listener offers all it enables.
        #[arg(long = "alpn")]
        alpn: Vec<String>,
    },
    /// Removes a profile nothing uses.
    Delete { id: String },
}

#[derive(Subcommand)]
pub(crate) enum ConfigCommand {
    /// The draft's version and whether the gateway runs it.
    Draft,
    /// Validates the draft, or only the given sites.
    Validate {
        #[arg(long = "site")]
        sites: Vec<String>,
    },
    /// Compiles the draft and activates it on the gateway.
    Apply {
        /// Refuse if the draft changed since this version.
        #[arg(long)]
        expected_version: Option<u64>,
    },
}

#[derive(Subcommand)]
pub(crate) enum GatewayCommand {
    /// Versions, uptime, workers, listeners and the active revision.
    Status,
    /// Starts a new listener generation and drains the previous one.
    Reload,
    /// Sets the number of data plane workers.
    Workers { count: u32 },
    /// Drains in-flight requests and stops the gateway.
    Shutdown {
        #[arg(long)]
        yes: bool,
    },
}

const LISTENERS: &[Column] = &[
    ("ID", |listener| text(&listener["id"])),
    ("ADDRESS", |listener| text(&listener["address"])),
    ("TLS", |listener| text(&listener["tls_profile_id"])),
    ("HTTP/1.1", |listener| text(&listener["protocols"]["http1"])),
    ("HTTP/2", |listener| text(&listener["protocols"]["http2"])),
    ("HTTP/3", |listener| text(&listener["protocols"]["http3"])),
    ("DEFAULT SITE", |listener| {
        text(&listener["default_site_id"])
    }),
];

const PROFILES: &[Column] = &[
    ("ID", |profile| text(&profile["id"])),
    ("CERTIFICATE", |profile| {
        text(&profile["certificate_secret_id"])
    }),
    ("KEY", |profile| text(&profile["private_key_secret_id"])),
    ("MIN TLS", |profile| text(&profile["min_protocol"])),
    ("ALPN", |profile| text(&profile["alpn"])),
];

/// The current entity tag of a resource, or none when it does not exist yet.
async fn existing(api: &Api, path: &str) -> Result<Option<String>> {
    match api.get(path, &[]).await {
        Ok(reply) => Ok(reply.etag),
        Err(CliError::Api { status, .. }) if status.as_u16() == 404 => Ok(None),
        Err(error) => Err(error),
    }
}

pub async fn listener(api: &Api, output: &Output, command: ListenerCommand) -> Result<()> {
    match command {
        ListenerCommand::List => {
            output.list(&api.get("/api/v1/listeners", &[]).await?.body, LISTENERS)
        }
        ListenerCommand::Show { id } => output.item(
            &api.get(&format!("/api/v1/listeners/{id}"), &[]).await?.body,
            LISTENERS,
        ),
        ListenerCommand::Set {
            id,
            address,
            tls_profile,
            no_http1,
            no_http2,
            http3,
            reuse_port,
            ipv6_only,
            default_site,
        } => {
            let path = format!("/api/v1/listeners/{id}");
            let etag = existing(api, &path).await?;
            let body = json!({
                "id": id,
                "address": address,
                "tls_profile_id": tls_profile,
                "protocols": {"http1": !no_http1, "http2": !no_http2, "http3": http3},
                "reuse_port": reuse_port,
                "ipv6_only": ipv6_only,
                "default_site_id": default_site,
            });
            let listener = api
                .change(Method::PUT, &path, Some(&body), etag.as_deref())
                .await?
                .body;
            output.done(&format!("Saved listener {id} on {address}"), &listener);
        }
        ListenerCommand::Delete { id } => {
            let path = format!("/api/v1/listeners/{id}");
            let etag = existing(api, &path).await?;
            let reply = api
                .change(Method::DELETE, &path, None, etag.as_deref())
                .await?
                .body;
            output.done(&format!("Deleted listener {id}"), &reply);
        }
    }
    Ok(())
}

pub async fn tls_profile(api: &Api, output: &Output, command: TlsProfileCommand) -> Result<()> {
    match command {
        TlsProfileCommand::List => {
            output.list(&api.get("/api/v1/tls-profiles", &[]).await?.body, PROFILES)
        }
        TlsProfileCommand::Show { id } => output.item(
            &api.get(&format!("/api/v1/tls-profiles/{id}"), &[])
                .await?
                .body,
            PROFILES,
        ),
        TlsProfileCommand::Set {
            id,
            certificate,
            key,
            min_protocol,
            alpn,
        } => {
            let path = format!("/api/v1/tls-profiles/{id}");
            let etag = existing(api, &path).await?;
            let body = json!({
                "id": id,
                "certificate_secret_id": certificate,
                "private_key_secret_id": key,
                "min_protocol": min_protocol,
                "alpn": alpn,
            });
            let profile = api
                .change(Method::PUT, &path, Some(&body), etag.as_deref())
                .await?
                .body;
            output.done(&format!("Saved TLS profile {id}"), &profile);
        }
        TlsProfileCommand::Delete { id } => {
            let path = format!("/api/v1/tls-profiles/{id}");
            let etag = existing(api, &path).await?;
            let reply = api
                .change(Method::DELETE, &path, None, etag.as_deref())
                .await?
                .body;
            output.done(&format!("Deleted TLS profile {id}"), &reply);
        }
    }
    Ok(())
}

pub async fn config(api: &Api, output: &Output, command: ConfigCommand) -> Result<()> {
    match command {
        ConfigCommand::Draft => {
            let draft = api.get("/api/v1/config/draft", &[]).await?.body;
            output.item(
                &draft,
                &[
                    ("Version", |draft| text(&draft["version"])),
                    ("Updated", |draft| text(&draft["updated_at"])),
                    ("Applied version", |draft| text(&draft["applied_version"])),
                    ("Applied", |draft| text(&draft["applied_at"])),
                    ("Pending changes", |draft| text(&draft["pending"])),
                ],
            );
        }
        ConfigCommand::Validate { sites } => {
            let query: Vec<(&str, String)> = if sites.is_empty() {
                Vec::new()
            } else {
                vec![("site_ids", sites.join(","))]
            };
            let result = api.get("/api/v1/config/validation", &query).await?.body;
            output.list(
                &result["diagnostics"],
                &[
                    ("RESOURCE", |diagnostic| text(&diagnostic["resource_id"])),
                    ("MESSAGE", |diagnostic| text(&diagnostic["message"])),
                ],
            );
            if result["valid"] != true {
                return Err(CliError::Api {
                    status: reqwest::StatusCode::BAD_REQUEST,
                    problem: json!({"detail": "the configuration has errors"}),
                });
            }
            output.done("The configuration is valid", &result);
        }
        ConfigCommand::Apply { expected_version } => {
            let applied = api
                .change(
                    Method::POST,
                    "/api/v1/config/apply",
                    Some(&json!({ "expected_version": expected_version })),
                    None,
                )
                .await?
                .body;
            output.done(
                &format!(
                    "Applied version {} (revision {}, {})",
                    text(&applied["draft"]["version"]),
                    text(&applied["revision_id"]),
                    text(&applied["content_hash"])
                ),
                &applied,
            );
        }
    }
    Ok(())
}

const DATA_PLANE: &[Column] = &[
    ("Gateway", |state| text(&state["gateway_version"])),
    ("Engine", |state| {
        format!("Pingora {}", text(&state["engine_version"]))
    }),
    ("Started", |state| text(&state["started_at"])),
    ("Uptime (s)", |state| text(&state["uptime_seconds"])),
    ("Workers", |state| text(&state["worker_count"])),
    ("Generation", |state| text(&state["generation"])),
    ("Listeners", |state| {
        state["listeners"]
            .as_array()
            .map(|listeners| {
                listeners
                    .iter()
                    .map(|listener| {
                        format!("{} {}", text(&listener["id"]), text(&listener["address"]))
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default()
    }),
    ("Active revision", |state| {
        text(&state["active_revision_id"])
    }),
    ("Error", |state| text(&state["error"])),
    ("Observed", |state| text(&state["observed_at"])),
];

pub async fn gateway(api: &Api, output: &Output, command: GatewayCommand) -> Result<()> {
    match command {
        GatewayCommand::Status => {
            output.item(
                &api.get("/api/v1/gateway/data-plane", &[]).await?.body,
                DATA_PLANE,
            );
        }
        GatewayCommand::Reload => {
            let state = api
                .change(Method::POST, "/api/v1/gateway/reload", None, None)
                .await?
                .body;
            output.done(
                &format!("Reloaded; generation {}", text(&state["generation"])),
                &state,
            );
        }
        GatewayCommand::Workers { count } => {
            let state = api
                .change(
                    Method::PUT,
                    "/api/v1/gateway/workers",
                    Some(&json!({ "worker_count": count })),
                    None,
                )
                .await?
                .body;
            output.done(
                &format!("Running {} workers", text(&state["worker_count"])),
                &state,
            );
        }
        GatewayCommand::Shutdown { yes } => {
            if !yes {
                return Err(CliError::Usage(
                    "shutting the gateway down stops all proxied traffic; pass --yes to confirm"
                        .into(),
                ));
            }
            let reply: Value = api
                .change(Method::POST, "/api/v1/gateway/shutdown", None, None)
                .await?
                .body;
            output.done("The gateway is draining and will stop", &reply);
        }
    }
    Ok(())
}
