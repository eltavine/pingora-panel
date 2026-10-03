//! TLS checks: a handshake with a configured HTTPS listener, made as a
//! client would make it, and what that client sees.

use crate::{configuration, error::ApiError, request_context::request_scope, ApiState};
use axum::{
    extract::{rejection::JsonRejection, State},
    http::HeaderMap,
    Json,
};
use chrono::Utc;
use panel_application::{ConfigurationRead, TlsProbe, TlsProbeTarget};
use panel_certificates::{describe_der, CertificateDetails, CertificateStatus};
use panel_config_model::Listener;
use panel_domain::NormalizedHost;
use panel_errors::PanelError;
use serde::{Deserialize, Serialize};
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    sync::Arc,
};
use utoipa::ToSchema;

fn probe<U>(state: &ApiState<U>) -> Result<Arc<dyn TlsProbe>, ApiError> {
    state
        .tls_probe
        .clone()
        .ok_or_else(|| ApiError::new(PanelError::unavailable("TLS checks are not available here")))
}

/// A listener and the host to ask it for.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TlsCheckRequest {
    /// A listener that serves HTTPS.
    listener: String,
    /// Sent as the server name and as the HTTP host.
    host: String,
}

/// Whether a version is accepted when a client offers it alone.
#[derive(Serialize, ToSchema)]
pub(crate) struct VersionAcceptance {
    version: String,
    accepted: bool,
}

/// What a client sees when it connects to the listener for the host.
#[derive(Serialize, ToSchema)]
pub(crate) struct TlsCheck {
    listener: String,
    /// The address connected to.
    address: String,
    host: String,
    /// Negotiated when every supported version is offered.
    protocol: String,
    cipher_suite: String,
    alpn: Option<String>,
    handshake_ms: u64,
    versions: Vec<VersionAcceptance>,
    /// The presented certificate, when it could be read.
    certificate: Option<CertificateDetails>,
    certificate_status: Option<CertificateStatus>,
    /// Whether the presented certificate names the host.
    covers_host: bool,
    /// The status of `HEAD /` when the listener speaks HTTP/1.1.
    http_status: Option<u16>,
    strict_transport_security: Option<String>,
}

/// The address a client on this host reaches a listener at.
fn reachable(address: SocketAddr) -> SocketAddr {
    match address.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), address.port())
        }
        IpAddr::V6(ip) if ip.is_unspecified() => {
            SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), address.port())
        }
        _ => address,
    }
}

/// Connects to a configured HTTPS listener for a host and reports the
/// negotiated version, cipher suite and ALPN, the versions accepted alone,
/// the certificate presented and whether it covers the host, and the
/// Strict-Transport-Security sent.
#[utoipa::path(post, path = "/api/v1/tls-checks", request_body = TlsCheckRequest,
    responses((status = 200, body = TlsCheck)), tag = "gateway")]
pub(crate) async fn check_tls<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    payload: Result<Json<TlsCheckRequest>, JsonRejection>,
) -> Result<Json<TlsCheck>, ApiError> {
    let Json(request) = payload.map_err(ApiError::from_json)?;
    let host = NormalizedHost::new(&request.host)
        .ok()
        .filter(|host| !host.is_wildcard())
        .ok_or_else(|| {
            ApiError::new(PanelError::invalid_argument(format!(
                "{:?} is not a host name",
                request.host
            )))
        })?;
    let probe = probe(&state)?;
    let scope = request_scope(&headers)?;
    let output = configuration::port(&state)?
        .read(
            scope.clone(),
            ConfigurationRead {
                operation: "listeners.get".into(),
                resource: format!("listeners/{}", request.listener),
                parameters: Vec::new(),
            },
        )
        .await?;
    let listener: Listener = serde_json::from_slice(&output.content).map_err(|_| {
        ApiError::new(PanelError::internal(
            "the configuration answered with an unreadable listener",
        ))
    })?;
    if listener.tls_profile_id.is_none() {
        return Err(ApiError::new(PanelError::invalid_argument(format!(
            "listener {} does not serve HTTPS",
            request.listener
        ))));
    }
    let address = listener
        .address
        .parse::<SocketAddr>()
        .map(reachable)
        .map_err(|_| {
            ApiError::new(PanelError::invalid_argument(format!(
                "listener {} has no IP socket address",
                request.listener
            )))
        })?;
    let report = probe
        .probe(
            scope,
            TlsProbeTarget {
                address,
                server_name: host.to_string(),
            },
        )
        .await?;
    let certificate = describe_der(&report.chain).ok();
    Ok(Json(TlsCheck {
        listener: request.listener,
        address: address.to_string(),
        covers_host: certificate
            .as_ref()
            .is_some_and(|details| details.covers(&host)),
        certificate_status: certificate
            .as_ref()
            .map(|details| details.status(Utc::now())),
        certificate,
        host: host.to_string(),
        protocol: report.protocol,
        cipher_suite: report.cipher_suite,
        alpn: report.alpn,
        handshake_ms: u64::try_from(report.handshake.as_millis()).unwrap_or(u64::MAX),
        versions: report
            .versions
            .into_iter()
            .map(|(version, accepted)| VersionAcceptance { version, accepted })
            .collect(),
        http_status: report.http_status,
        strict_transport_security: report.strict_transport_security,
    }))
}
