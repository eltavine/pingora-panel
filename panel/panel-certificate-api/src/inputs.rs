//! What changes to ACME accounts, automatic certificates and DNS providers
//! take.

use crate::Secret;
use panel_domain::CertificateId;
use panel_errors::{PanelError, Result};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Names an ACME account: 1 to 64 lowercase letters, digits and hyphens.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AccountId(String);

impl AccountId {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        let valid = (1..=64).contains(&value.len())
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            && !value.starts_with('-')
            && !value.ends_with('-');
        if !valid {
            return Err(PanelError::invalid_argument(format!(
                "{value:?} is not an account ID: use 1 to 64 lowercase letters, digits and hyphens"
            )));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AccountId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for AccountId {
    type Error = PanelError;

    fn try_from(value: String) -> Result<Self> {
        Self::new(value)
    }
}

impl From<AccountId> for String {
    fn from(value: AccountId) -> Self {
        value.0
    }
}

/// The key identifier and MAC key a CA hands out for registering with an
/// external account binding (RFC 8555 §7.3.4). The MAC key is used once.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalAccount {
    pub key_id: String,
    /// Base64url-encoded, as the CA shows it.
    pub mac_key: Secret,
}

/// An account to register with an ACME CA.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewAccount {
    pub id: AccountId,
    /// The CA's directory URL.
    pub directory: String,
    /// PEM roots to trust for a private directory instead of the system's.
    #[serde(default)]
    pub ca_bundle: Option<String>,
    #[serde(default)]
    pub contact: Vec<String>,
    /// Registering accepts the CA's terms of service.
    #[serde(default)]
    pub terms_of_service_agreed: bool,
    #[serde(default)]
    pub external_account: Option<ExternalAccount>,
}

/// How a CA checks control of a certificate's names (RFC 8555 §8).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum Challenge {
    #[default]
    #[serde(rename = "http-01")]
    Http01,
    #[serde(rename = "dns-01")]
    Dns01,
}

/// A certificate to obtain from an ACME CA and keep renewed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewAutomaticCertificate {
    /// The inventory certificate it produces.
    pub id: CertificateId,
    pub account: AccountId,
    pub names: Vec<String>,
    #[serde(default)]
    pub challenge: Challenge,
    /// The DNS provider that publishes the records of DNS-01, which needs
    /// one or a plugin.
    #[serde(default)]
    pub dns_provider: Option<String>,
    /// The plugin whose DNS-01 port publishes the records, in place of a
    /// DNS provider.
    #[serde(default)]
    pub dns_plugin: Option<String>,
}

/// The kind of a DNS provider.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DnsProviderKind {
    /// Dynamic updates (RFC 2136) signed with TSIG (RFC 8945).
    Rfc2136,
}

impl DnsProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rfc2136 => "rfc2136",
        }
    }
}

/// A TSIG algorithm (RFC 8945).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum TsigAlgorithm {
    #[serde(rename = "hmac-sha256")]
    HmacSha256,
    #[serde(rename = "hmac-sha512")]
    HmacSha512,
}

impl TsigAlgorithm {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HmacSha256 => "hmac-sha256",
            Self::HmacSha512 => "hmac-sha512",
        }
    }
}

/// Where an RFC 2136 provider sends updates and the key it signs them with.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rfc2136Config {
    /// The primary server as `host:port`.
    pub server: String,
    /// The zones the key may update.
    pub zones: Vec<String>,
    pub key_name: String,
    pub algorithm: TsigAlgorithm,
    #[serde(default)]
    pub ttl: Option<u32>,
}

/// A DNS provider to keep.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewDnsProvider {
    pub id: String,
    pub kind: DnsProviderKind,
    pub rfc2136: Rfc2136Config,
    /// The TSIG secret, base64-encoded.
    pub secret: Secret,
    /// Seconds a record takes to reach every authoritative server; the
    /// service's default when absent.
    #[serde(default)]
    pub propagation_seconds: Option<u32>,
}

/// New settings for a DNS provider; its secret stays unless a new one is
/// given.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DnsProviderChange {
    pub rfc2136: Rfc2136Config,
    #[serde(default)]
    pub secret: Option<Secret>,
    #[serde(default)]
    pub propagation_seconds: Option<u32>,
}
