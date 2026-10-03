//! The editable configuration document.

use crate::security::SecurityPolicy;
use chrono::{DateTime, Utc};
use panel_domain::{CertificateId, ContentHash, NormalizedHost};
use panel_ir::{
    ActiveHealthCheck, ListenerProtocols, LoadBalancingPolicy, PassiveHealthPolicy, RealIpHeader,
    StrictTransportSecurity, UpstreamConnectionPolicy, UpstreamTlsPolicy, WwwRedirect,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use uuid::Uuid;

const fn enabled() -> bool {
    true
}

const fn one() -> u32 {
    1
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// Everything an operator configures for one gateway.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ConfigModel {
    #[serde(default)]
    pub listeners: Vec<Listener>,
    #[serde(default)]
    pub tls_profiles: Vec<TlsProfile>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub security_policies: Vec<SecurityPolicy>,
    #[serde(default)]
    pub upstreams: Vec<Upstream>,
    #[serde(default)]
    pub sites: Vec<Site>,
}

/// A certificate and the TLS settings listeners and hosts serve it with.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct TlsProfile {
    pub id: String,
    /// A certificate of the inventory, which the panel delivers to the gateway.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate_id: Option<CertificateId>,
    /// The PEM chain's file in the gateway's secret directory; empty when
    /// `certificate_id` names the certificate.
    pub certificate_secret_id: String,
    /// The PEM private key's file in the gateway's secret directory; empty
    /// when `certificate_id` names the certificate.
    pub private_key_secret_id: String,
    pub min_protocol: String,
    /// The newest TLS version accepted; the newest supported when absent.
    /// Like the cipher suites and session resumption, it applies to listeners
    /// that use this profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_protocol: Option<String>,
    /// IANA names of the cipher suites accepted; the defaults when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cipher_suites: Vec<String>,
    #[serde(default = "enabled")]
    pub session_resumption: bool,
    /// Reserved: recorded, but OCSP responses are not stapled yet.
    #[serde(default)]
    pub ocsp_stapling: bool,
    /// ALPN protocol IDs a listener using this profile offers, narrowing the
    /// listener's enabled protocols; empty offers all of them. A profile chosen
    /// by SNI for a domain does not change its listener's offer.
    pub alpn: BTreeSet<String>,
}

impl TlsProfile {
    /// Whether listeners using it need settings beyond a minimum version.
    pub fn narrows_listener(&self) -> bool {
        self.max_protocol.is_some() || !self.cipher_suites.is_empty() || !self.session_resumption
    }

    /// The files the gateway reads the chain and key from: those delivered
    /// for its certificate, or those it names.
    pub fn secret_files(&self) -> (String, String) {
        match &self.certificate_id {
            Some(id) => (id.chain_file(), id.key_file()),
            None => (
                self.certificate_secret_id.clone(),
                self.private_key_secret_id.clone(),
            ),
        }
    }

    pub fn runtime(&self) -> panel_ir::TlsProfile {
        let (certificate_secret_id, private_key_secret_id) = self.secret_files();
        panel_ir::TlsProfile {
            id: self.id.clone(),
            certificate_secret_id,
            private_key_secret_id,
            min_protocol: self.min_protocol.clone(),
            max_protocol: self.max_protocol.clone(),
            cipher_suites: self.cipher_suites.clone(),
            session_resumption: self.session_resumption,
            alpn: self.alpn.clone(),
        }
    }
}

fn tls12() -> String {
    "TLSv1.2".into()
}

/// A TLS profile as written: a certificate of the inventory, or a chain and
/// key placed in the gateway's secret directory.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct TlsProfileInput {
    pub id: String,
    #[serde(default)]
    pub certificate_id: Option<CertificateId>,
    #[serde(default)]
    pub certificate_secret_id: String,
    #[serde(default)]
    pub private_key_secret_id: String,
    /// `TLSv1.2` or `TLSv1.3`; `TLSv1.2` when absent.
    #[serde(default = "tls12")]
    pub min_protocol: String,
    #[serde(default)]
    pub max_protocol: Option<String>,
    #[serde(default)]
    pub cipher_suites: Vec<String>,
    #[serde(default = "enabled")]
    pub session_resumption: bool,
    #[serde(default)]
    pub ocsp_stapling: bool,
    #[serde(default)]
    pub alpn: BTreeSet<String>,
}

impl From<TlsProfileInput> for TlsProfile {
    fn from(input: TlsProfileInput) -> Self {
        Self {
            id: input.id,
            certificate_id: input.certificate_id,
            certificate_secret_id: input.certificate_secret_id,
            private_key_secret_id: input.private_key_secret_id,
            min_protocol: input.min_protocol,
            max_protocol: input.max_protocol,
            cipher_suites: input.cipher_suites,
            session_resumption: input.session_resumption,
            ocsp_stapling: input.ocsp_stapling,
            alpn: input.alpn,
        }
    }
}

/// A fixed listening socket shared by the sites it serves.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Listener {
    /// Stable identifier such as `http` or `https-v6`.
    pub id: String,
    /// `ip:port`, for example `0.0.0.0:443` or `[::]:443`.
    pub address: String,
    /// Serves HTTPS with this profile's certificate when no domain claims one.
    #[serde(default)]
    pub tls_profile_id: Option<String>,
    #[serde(default)]
    pub protocols: ListenerProtocols,
    #[serde(default)]
    pub reuse_port: bool,
    #[serde(default)]
    pub ipv6_only: Option<bool>,
    /// Serves requests for unknown hosts; they are rejected with 421 otherwise.
    #[serde(default)]
    pub default_site_id: Option<Uuid>,
    /// Proxies in CIDR notation whose forwarding headers name the client.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trusted_proxies: Vec<String>,
    /// The header trusted proxies name the client in.
    #[serde(default, skip_serializing_if = "is_default")]
    pub real_ip_header: RealIpHeader,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Site {
    pub id: Uuid,
    pub name: String,
    /// What the site does for requests no route claims; also its type.
    pub action: Action,
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub domains: Vec<Domain>,
    /// Evaluated by priority before the site action.
    #[serde(default)]
    pub routes: Vec<Route>,
    /// Listeners serving the site; empty means all of them.
    #[serde(default)]
    pub listener_ids: BTreeSet<String>,
    #[serde(default)]
    pub https_redirect: bool,
    #[serde(default)]
    pub www_redirect: WwwRedirect,
    /// Certificate for domains that do not name their own.
    #[serde(default)]
    pub tls_profile_id: Option<String>,
    /// Sent with HTTPS responses for the site's hosts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hsts: Option<StrictTransportSecurity>,
    /// Restrictions every request to the site passes first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security_policy_id: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub tags: BTreeSet<String>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub favorite: bool,
    /// Set while the site is in the recycle bin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Site {
    pub fn kind(&self) -> SiteKind {
        match self.action {
            Action::Proxy { .. } => SiteKind::ReverseProxy,
            Action::Static { .. } => SiteKind::Static,
            Action::Redirect { .. } => SiteKind::Redirect,
            Action::Respond { .. } => SiteKind::Maintenance,
        }
    }

    pub fn is_deleted(&self) -> bool {
        self.deleted_at.is_some()
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SiteKind {
    ReverseProxy,
    Static,
    Redirect,
    Maintenance,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Domain {
    pub host: NormalizedHost,
    #[serde(default = "enabled")]
    pub enabled: bool,
    /// Target of alias and `www` redirects.
    #[serde(default)]
    pub primary: bool,
    /// Redirects to the primary domain instead of serving the site.
    #[serde(default)]
    pub redirect: bool,
    #[serde(default)]
    pub tls_profile_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Route {
    pub id: Uuid,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default = "enabled")]
    pub enabled: bool,
    /// Lower values are evaluated first.
    pub priority: u32,
    #[serde(rename = "match")]
    pub matcher: RouteMatch,
    pub action: Action,
    /// Restrictions the route's requests pass after the site's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub security_policy_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct RouteMatch {
    pub kind: MatchKind,
    /// An absolute path, glob or regular expression, depending on `kind`.
    pub path: String,
    /// Restricts a prefix route to one of the site's hosts.
    #[serde(default)]
    pub host: Option<NormalizedHost>,
}

/// Explicit matcher kinds; there is no implicit precedence between them
/// beyond the documented tie-break for equal priorities.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum MatchKind {
    Exact,
    Prefix,
    Glob,
    Regex,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[non_exhaustive]
pub enum Action {
    Proxy {
        upstream_id: Uuid,
    },
    Static {
        /// Directory below the gateway's static root.
        root: String,
        #[serde(default = "default_index_files")]
        index_files: Vec<String>,
        #[serde(default)]
        spa_fallback: bool,
    },
    Redirect {
        location: String,
        #[serde(default = "default_redirect_status")]
        status: u16,
        #[serde(default = "enabled")]
        preserve_path: bool,
    },
    Respond {
        #[serde(default = "default_respond_status")]
        status: u16,
        #[serde(default)]
        body: Option<String>,
        #[serde(default)]
        content_type: Option<String>,
        #[serde(default)]
        retry_after_seconds: Option<u32>,
    },
}

fn default_index_files() -> Vec<String> {
    vec!["index.html".into()]
}

const fn default_redirect_status() -> u16 {
    308
}

const fn default_respond_status() -> u16 {
    503
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Upstream {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub nodes: Vec<UpstreamNode>,
    #[serde(default = "round_robin")]
    pub balancing: LoadBalancingPolicy,
    /// Replaces the client's `Host` when forwarding.
    #[serde(default)]
    pub host_header: Option<String>,
    #[serde(default)]
    pub tls: UpstreamTlsPolicy,
    #[serde(default)]
    pub connection: UpstreamConnectionPolicy,
    #[serde(default)]
    pub health_check: Option<ActiveHealthCheck>,
    #[serde(default)]
    pub passive_health: Option<PassiveHealthPolicy>,
    #[serde(default)]
    pub note: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn round_robin() -> LoadBalancingPolicy {
    LoadBalancingPolicy::RoundRobin
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct UpstreamNode {
    pub id: Uuid,
    /// IP literal or DNS name, resolved when a configuration is applied.
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub tls: bool,
    #[serde(default = "one")]
    pub weight: u32,
    #[serde(default = "enabled")]
    pub enabled: bool,
    /// Serves only while no primary node is available.
    #[serde(default)]
    pub backup: bool,
    #[serde(default)]
    pub sni: Option<String>,
    /// Reserved: Unix domain socket path.
    #[serde(default)]
    pub unix_socket: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

/// Strong entity tag over a resource's canonical JSON (RFC 9110 §8.8.3).
pub fn entity_tag(resource: &impl Serialize) -> String {
    let bytes = serde_json::to_vec(resource).expect("configuration resources serialize");
    format!("\"{}\"", ContentHash::from_bytes(&bytes).as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn documents_decode_with_defaults_and_reject_unknown_fields() {
        let site: Site = serde_json::from_value(serde_json::json!({
            "id": "0190b5b6-3f43-7a52-8a56-2f8b7a7d5a10",
            "name": "shop",
            "action": {"type": "redirect", "location": "https://example.com"},
            "domains": [{"host": "Shop.Example.COM"}],
            "created_at": "2026-10-01T00:00:00Z",
            "updated_at": "2026-10-01T00:00:00Z"
        }))
        .unwrap();
        assert!(site.enabled);
        assert_eq!(site.kind(), SiteKind::Redirect);
        assert_eq!(site.domains[0].host.as_str(), "shop.example.com");
        assert_eq!(
            site.action,
            Action::Redirect {
                location: "https://example.com".into(),
                status: 308,
                preserve_path: true
            }
        );
        let mut value = serde_json::to_value(&site).unwrap();
        value["unexpected"] = serde_json::json!(true);
        assert!(serde_json::from_value::<Site>(value).is_err());
    }

    #[test]
    fn entity_tags_follow_content() {
        let model = ConfigModel::default();
        let mut changed = model.clone();
        changed.listeners.push(Listener {
            id: "http".into(),
            address: "0.0.0.0:80".into(),
            tls_profile_id: None,
            protocols: ListenerProtocols::default(),
            reuse_port: false,
            ipv6_only: None,
            default_site_id: None,
            real_ip_header: Default::default(),
            trusted_proxies: Default::default(),
        });
        assert_eq!(entity_tag(&model), entity_tag(&ConfigModel::default()));
        assert_ne!(entity_tag(&model), entity_tag(&changed));
        assert!(entity_tag(&model).starts_with('"'));
    }
}
