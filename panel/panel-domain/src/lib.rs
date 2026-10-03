#![forbid(unsafe_code)]

//! Domain value objects. This crate deliberately has no transport or engine dependency.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fmt, net::IpAddr};
use thiserror::Error;

#[derive(Debug, Error, Clone, Eq, PartialEq)]
pub enum DomainError {
    #[error("value must not be empty")]
    Empty,
    #[error("value exceeds maximum length of {0}")]
    TooLong(usize),
    #[error("invalid value: {0}")]
    Invalid(String),
}

macro_rules! typed_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
                let value = value.into();
                validate_token(&value)?;
                Ok(Self(value))
            }
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: serde::Deserializer<'de>,
            {
                Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
            }
        }
    };
}

fn validate_token(value: &str) -> Result<(), DomainError> {
    if value.is_empty() {
        return Err(DomainError::Empty);
    }
    if value.len() > 128 {
        return Err(DomainError::TooLong(128));
    }
    if !value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    {
        return Err(DomainError::Invalid(
            "only ASCII letters, digits, '.', '_' and '-' are allowed".into(),
        ));
    }
    Ok(())
}

typed_id!(SiteId);
typed_id!(RouteId);
typed_id!(UpstreamPoolId);
typed_id!(EndpointId);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RevisionId(u64);

impl RevisionId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for RevisionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct ContentHash(String);

impl ContentHash {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        Self(hex::encode(Sha256::digest(bytes)))
    }

    pub fn from_hex(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(DomainError::Invalid(
                "content hash must be 64 hexadecimal characters".into(),
            ));
        }
        Ok(Self(value.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for ContentHash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::from_hex(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// A DNS host name in canonical A-label form, or a wildcard pattern whose
/// leftmost label is `*` (RFC 6125 §6.4.3 without partial-label wildcards).
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema), schema(value_type = String, example = "example.com"))]
#[serde(transparent)]
pub struct NormalizedHost(String);

const WILDCARD_PREFIX: &str = "*.";

impl NormalizedHost {
    /// Internationalized names are converted with UTS #46 so equal names have
    /// one representation regardless of how they were entered.
    pub fn new(value: impl AsRef<str>) -> Result<Self, DomainError> {
        let value = value.as_ref().trim();
        let value = value.strip_suffix('.').unwrap_or(value);
        if value.is_empty() {
            return Err(DomainError::Empty);
        }
        let (wildcard, name) = match value.strip_prefix(WILDCARD_PREFIX) {
            Some(name) => (true, name),
            None => (false, value),
        };
        if name.parse::<IpAddr>().is_ok() {
            return Err(DomainError::Invalid(
                "host must be a DNS name, not an IP literal".into(),
            ));
        }
        let ascii = idna::domain_to_ascii_strict(name)
            .map_err(|_| DomainError::Invalid("invalid DNS host name".into()))?;
        for label in ascii.split('.') {
            if label.is_empty()
                || label.len() > 63
                || label.starts_with('-')
                || label.ends_with('-')
                || !label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            {
                return Err(DomainError::Invalid("invalid DNS host label".into()));
            }
        }
        if wildcard && !ascii.contains('.') {
            return Err(DomainError::Invalid(
                "a wildcard must cover at least two labels".into(),
            ));
        }
        let value = if wildcard {
            format!("{WILDCARD_PREFIX}{ascii}")
        } else {
            ascii
        };
        if value.len() > 253 {
            return Err(DomainError::TooLong(253));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_wildcard(&self) -> bool {
        self.0.starts_with(WILDCARD_PREFIX)
    }

    /// A wildcard matches exactly one additional leftmost label.
    pub fn matches(&self, host: &NormalizedHost) -> bool {
        match self.0.strip_prefix('*') {
            Some(suffix) => {
                !host.is_wildcard()
                    && host
                        .0
                        .strip_suffix(suffix)
                        .is_some_and(|label| !label.is_empty() && !label.contains('.'))
            }
            None => self == host,
        }
    }

    /// The name with U-labels for display; A-labels are kept where they do not decode.
    pub fn to_unicode(&self) -> String {
        let (unicode, result) = idna::domain_to_unicode(&self.0);
        if result.is_ok() {
            unicode
        } else {
            self.0.clone()
        }
    }
}

impl fmt::Display for NormalizedHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for NormalizedHost {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct PathPrefix(String);

impl PathPrefix {
    pub fn new(value: impl AsRef<str>) -> Result<Self, DomainError> {
        let value = value.as_ref().trim();
        if value.is_empty()
            || !value.starts_with('/')
            || value.contains('\\')
            || value.contains("..")
        {
            return Err(DomainError::Invalid(
                "path prefix must be an absolute normalized path".into(),
            ));
        }
        if value.len() > 2048 {
            return Err(DomainError::TooLong(2048));
        }
        let normalized = if value.len() > 1 {
            value.trim_end_matches('/')
        } else {
            value
        };
        Ok(Self(normalized.to_string()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PathPrefix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for PathPrefix {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct EndpointAddress {
    host: String,
    port: u16,
    tls: bool,
}

impl EndpointAddress {
    pub fn new(host: impl AsRef<str>, port: u16, tls: bool) -> Result<Self, DomainError> {
        let host = host.as_ref().trim();
        if host.is_empty() || host.len() > 253 || host.contains(char::is_whitespace) {
            return Err(DomainError::Invalid("invalid endpoint host".into()));
        }
        if port == 0 {
            return Err(DomainError::Invalid(
                "endpoint port must be non-zero".into(),
            ));
        }
        let normalized_host = host.trim_start_matches('[').trim_end_matches(']');
        if normalized_host.parse::<IpAddr>().is_err() {
            for label in normalized_host.split('.') {
                if label.is_empty()
                    || label.len() > 63
                    || label.starts_with('-')
                    || label.ends_with('-')
                    || !label
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                {
                    return Err(DomainError::Invalid("invalid endpoint DNS host".into()));
                }
            }
        }
        Ok(Self {
            host: normalized_host.to_ascii_lowercase(),
            port,
            tls,
        })
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub const fn port(&self) -> u16 {
        self.port
    }

    pub const fn tls(&self) -> bool {
        self.tls
    }
}

impl<'de> Deserialize<'de> for EndpointAddress {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            host: String,
            port: u16,
            tls: bool,
        }

        let fields = Fields::deserialize(deserializer)?;
        Self::new(fields.host, fields.port, fields.tls).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RevisionRef {
    pub revision_id: RevisionId,
    pub content_hash: ContentHash,
}

const MAX_CERTIFICATE_ID_LEN: usize = 64;

/// The subdirectory of the gateway's secret directory where HTTP-01 key
/// authorizations wait while certificates are issued, one file per token.
pub const ACME_CHALLENGE_DIRECTORY: &str = "acme-challenge";

/// Names a certificate of the inventory, such as `example.com` or
/// `intranet-wildcard`: lowercase letters, digits and hyphens in labels
/// separated by dots, so it is also a safe file name.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema), schema(value_type = String, example = "example.com"))]
#[serde(transparent)]
pub struct CertificateId(String);

impl CertificateId {
    pub fn new(value: impl AsRef<str>) -> Result<Self, DomainError> {
        let value = value.as_ref();
        if value.is_empty() {
            return Err(DomainError::Empty);
        }
        if value.len() > MAX_CERTIFICATE_ID_LEN {
            return Err(DomainError::TooLong(MAX_CERTIFICATE_ID_LEN));
        }
        let labels_valid = value.split('.').all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        });
        if !labels_valid {
            return Err(DomainError::Invalid(format!(
                "certificate id {value:?} must be lowercase letters, digits and hyphens in labels separated by dots"
            )));
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The chain's file in the gateway's secret directory.
    pub fn chain_file(&self) -> String {
        format!("cert-{}.pem", self.0)
    }

    /// The private key's file in the gateway's secret directory.
    pub fn key_file(&self) -> String {
        format!("cert-{}.key", self.0)
    }
}

impl fmt::Display for CertificateId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for CertificateId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_typed_and_validated() {
        assert!(SiteId::new("site-1").is_ok());
        assert!(SiteId::new("").is_err());
        assert!(SiteId::new("bad id").is_err());
    }

    #[test]
    fn host_and_path_are_normalized() {
        assert_eq!(
            NormalizedHost::new("Example.COM.").unwrap().as_str(),
            "example.com"
        );
        assert_eq!(PathPrefix::new("/api/").unwrap().as_str(), "/api");
        assert!(PathPrefix::new("relative").is_err());
    }

    #[test]
    fn internationalized_hosts_use_one_ascii_form() {
        let unicode = NormalizedHost::new("Bücher.Example").unwrap();
        let ascii = NormalizedHost::new("xn--bcher-kva.example").unwrap();
        assert_eq!(unicode, ascii);
        assert_eq!(unicode.as_str(), "xn--bcher-kva.example");
        assert_eq!(unicode.to_unicode(), "bücher.example");
        assert!(NormalizedHost::new("xn--a.example").is_err());
        assert!(NormalizedHost::new("under_score.example").is_err());
        assert!(NormalizedHost::new("example.com..").is_err());
        assert!(NormalizedHost::new("192.0.2.1").is_err());
    }

    #[test]
    fn wildcards_cover_exactly_one_label() {
        let wildcard = NormalizedHost::new("*.Example.com").unwrap();
        assert!(wildcard.is_wildcard());
        assert_eq!(wildcard.as_str(), "*.example.com");
        let host = |value| NormalizedHost::new(value).unwrap();
        assert!(wildcard.matches(&host("api.example.com")));
        assert!(!wildcard.matches(&host("example.com")));
        assert!(!wildcard.matches(&host("a.b.example.com")));
        assert!(!wildcard.matches(&host("api.example.org")));
        assert!(!wildcard.matches(&wildcard));
        assert!(host("example.com").matches(&host("example.com")));
        assert!(NormalizedHost::new("*.com").is_err());
        assert!(NormalizedHost::new("a.*.example.com").is_err());
        assert!(NormalizedHost::new("f*.example.com").is_err());
        assert_eq!(
            NormalizedHost::new("*.bücher.example").unwrap().as_str(),
            "*.xn--bcher-kva.example"
        );
    }

    #[test]
    fn sha256_is_lowercase_hex() {
        assert_eq!(
            ContentHash::from_bytes(b"abc").as_str(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn endpoint_address_validates_dns_and_ip_hosts() {
        assert!(EndpointAddress::new("127.0.0.1", 8080, false).is_ok());
        assert!(EndpointAddress::new("[::1]", 8080, false).is_ok());
        assert!(EndpointAddress::new("backend.example", 443, true).is_ok());
        assert!(EndpointAddress::new("bad host", 443, true).is_err());
    }

    #[test]
    fn deserialization_preserves_value_object_invariants() {
        assert!(serde_json::from_str::<SiteId>("\"bad id\"").is_err());
        assert!(serde_json::from_str::<ContentHash>("\"bad\"").is_err());
        assert!(serde_json::from_str::<NormalizedHost>("\"bad..host\"").is_err());
        assert!(serde_json::from_str::<PathPrefix>("\"relative\"").is_err());
        assert!(serde_json::from_str::<EndpointAddress>(
            r#"{"host":"127.0.0.1","port":0,"tls":false}"#
        )
        .is_err());
        assert_eq!(
            serde_json::from_str::<NormalizedHost>("\"Example.COM.\"")
                .unwrap()
                .as_str(),
            "example.com"
        );
    }

    #[test]
    fn certificate_ids_are_file_name_safe_labels() {
        for valid in ["example.com", "a", "intranet-wildcard", "0.example"] {
            let id = CertificateId::new(valid).unwrap();
            assert_eq!(id.chain_file(), format!("cert-{valid}.pem"));
            assert_eq!(id.key_file(), format!("cert-{valid}.key"));
        }
        for invalid in [
            "",
            "Example.com",
            "a..b",
            ".a",
            "a.",
            "-a",
            "a-",
            "a/b",
            "*.example.com",
            "a_b",
            &"a".repeat(65),
        ] {
            assert!(CertificateId::new(invalid).is_err(), "{invalid}");
        }
        assert!(serde_json::from_str::<CertificateId>("\"../etc\"").is_err());
    }
}
