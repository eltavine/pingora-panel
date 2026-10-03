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
        /// Network of proxies whose forwarding headers name the client, such
        /// as 10.0.0.0/8; repeatable. Other peers' forwarding headers are
        /// dropped.
        #[arg(long = "trusted-proxy", value_name = "CIDR")]
        trusted_proxies: Vec<String>,
        /// The header trusted proxies name the client in: x-forwarded-for,
        /// x-real-ip or forwarded.
        #[arg(long, requires = "trusted_proxies")]
        real_ip_header: Option<String>,
        /// The longest a client may take to send a request head; 30 when
        /// not given.
        #[arg(long, value_name = "SECONDS")]
        request_head_timeout: Option<u64>,
    },
    /// Removes a listener no site uses.
    Delete { id: String },
    /// Connects to an HTTPS listener for a host as a client would and shows
    /// what it sees.
    Check {
        id: String,
        /// The host to ask for.
        #[arg(long)]
        host: String,
    },
}

#[derive(Subcommand)]
pub(crate) enum TlsProfileCommand {
    /// Lists TLS profiles.
    List,
    /// Shows a TLS profile.
    Show { id: String },
    /// Creates or replaces a profile serving a certificate of the inventory
    /// or files placed in the gateway's secret directory.
    Set {
        id: String,
        /// A certificate of the inventory; see `ppanel certificate list`.
        #[arg(long, conflicts_with_all = ["certificate", "key"], required_unless_present = "certificate")]
        certificate_id: Option<String>,
        /// File name of the PEM certificate chain.
        #[arg(long, requires = "key")]
        certificate: Option<String>,
        /// File name of the PEM private key.
        #[arg(long, requires = "certificate")]
        key: Option<String>,
        #[arg(long, default_value = "TLSv1.2")]
        min_protocol: String,
        /// The newest TLS version accepted; the newest supported without it.
        #[arg(long)]
        max_protocol: Option<String>,
        /// A cipher suite accepted, by IANA name such as
        /// TLS13_AES_256_GCM_SHA384; repeatable. Without it, every supported
        /// suite.
        #[arg(long = "cipher")]
        ciphers: Vec<String>,
        /// Refuse resuming sessions by session IDs and tickets.
        #[arg(long)]
        no_session_resumption: bool,
        /// Reserved: recorded, but no OCSP responses are stapled yet.
        #[arg(long)]
        ocsp_stapling: bool,
        /// Protocol a listener using this profile offers through ALPN, h2 or
        /// http/1.1; repeatable. Without it the listener offers all it enables.
        #[arg(long = "alpn")]
        alpn: Vec<String>,
    },
    /// Removes a profile nothing uses.
    Delete { id: String },
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
        match profile["certificate_id"].as_str() {
            Some(id) => format!("{id} (inventory)"),
            None => text(&profile["certificate_secret_id"]),
        }
    }),
    ("KEY", |profile| text(&profile["private_key_secret_id"])),
    ("MIN TLS", |profile| text(&profile["min_protocol"])),
    ("MAX TLS", |profile| text(&profile["max_protocol"])),
    ("ALPN", |profile| text(&profile["alpn"])),
];

/// Each version and whether the listener accepts it alone.
fn versions(check: &Value) -> String {
    check["versions"]
        .as_array()
        .map(|versions| {
            versions
                .iter()
                .map(|version| {
                    format!(
                        "{} {}",
                        text(&version["version"]),
                        if version["accepted"] == true {
                            "yes"
                        } else {
                            "no"
                        }
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_else(|| "-".into())
}

const TLS_CHECK: &[Column] = &[
    ("ADDRESS", |check| text(&check["address"])),
    ("PROTOCOL", |check| text(&check["protocol"])),
    ("CIPHER", |check| text(&check["cipher_suite"])),
    ("ALPN", |check| text(&check["alpn"])),
    ("VERSIONS", versions),
    ("HANDSHAKE", |check| {
        format!("{} ms", text(&check["handshake_ms"]))
    }),
    ("CERTIFICATE", |check| {
        text(&check["certificate"]["subject"])
    }),
    ("NAMES", |check| text(&check["certificate"]["names"])),
    ("COVERS HOST", |check| text(&check["covers_host"])),
    ("STATUS", |check| text(&check["certificate_status"])),
    ("NOT AFTER", |check| {
        text(&check["certificate"]["not_after"])
    }),
    ("HSTS", |check| text(&check["strict_transport_security"])),
];

/// The current entity tag of a resource, or none when it does not exist yet.
pub(crate) async fn existing(api: &Api, path: &str) -> Result<Option<String>> {
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
            trusted_proxies,
            real_ip_header,
            request_head_timeout,
        } => {
            let path = format!("/api/v1/listeners/{id}");
            let etag = existing(api, &path).await?;
            let mut body = json!({
                "id": id,
                "address": address,
                "tls_profile_id": tls_profile,
                "protocols": {"http1": !no_http1, "http2": !no_http2, "http3": http3},
                "reuse_port": reuse_port,
                "ipv6_only": ipv6_only,
                "default_site_id": default_site,
                "trusted_proxies": trusted_proxies,
                "request_head_timeout_seconds": request_head_timeout,
            });
            if let Some(header) = real_ip_header {
                body["real_ip_header"] = json!(header.to_ascii_lowercase());
            }
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
        ListenerCommand::Check { id, host } => {
            let checked = api
                .post_read(
                    "/api/v1/tls-checks",
                    &json!({ "listener": id, "host": host }),
                )
                .await?
                .body;
            output.item(&checked, TLS_CHECK);
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
            certificate_id,
            certificate,
            key,
            min_protocol,
            max_protocol,
            ciphers,
            no_session_resumption,
            ocsp_stapling,
            alpn,
        } => {
            let path = format!("/api/v1/tls-profiles/{id}");
            let etag = existing(api, &path).await?;
            let body = json!({
                "id": id,
                "certificate_id": certificate_id,
                "certificate_secret_id": certificate.unwrap_or_default(),
                "private_key_secret_id": key.unwrap_or_default(),
                "min_protocol": min_protocol,
                "max_protocol": max_protocol,
                "cipher_suites": ciphers,
                "session_resumption": !no_session_resumption,
                "ocsp_stapling": ocsp_stapling,
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
