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
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentHash(String);

impl ContentHash {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let digest = Sha256::digest(bytes);
        Self(format!("{digest:x}"))
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

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NormalizedHost(String);

impl NormalizedHost {
    pub fn new(value: impl AsRef<str>) -> Result<Self, DomainError> {
        let value = value
            .as_ref()
            .trim()
            .trim_end_matches('.')
            .to_ascii_lowercase();
        if value.is_empty() {
            return Err(DomainError::Empty);
        }
        if value.len() > 253 {
            return Err(DomainError::TooLong(253));
        }
        if value.parse::<IpAddr>().is_ok() {
            return Err(DomainError::Invalid(
                "host must be a DNS name, not an IP literal".into(),
            ));
        }
        for label in value.split('.') {
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
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NormalizedHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RevisionRef {
    pub revision_id: RevisionId,
    pub content_hash: ContentHash,
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
}
