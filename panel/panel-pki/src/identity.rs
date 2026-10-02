use panel_context::ServiceName;
use panel_errors::{PanelError, Result};
use std::fmt;

/// The trust domain of an installation; `.internal` is reserved for
/// private use.
pub const DEFAULT_TRUST_DOMAIN: &str = "pingora-panel.internal";

/// A DNS name under which every identity of one installation is issued.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TrustDomain(String);

impl TrustDomain {
    /// Lowercase DNS labels of letters, digits and `-`, separated by dots.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        let label = |label: &str| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        };
        if value.len() > 200 || !value.contains('.') || !value.split('.').all(label) {
            return Err(PanelError::invalid_argument(format!(
                "trust domain `{value}` must be a lowercase DNS name"
            )));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for TrustDomain {
    fn default() -> Self {
        Self(DEFAULT_TRUST_DOMAIN.into())
    }
}

impl fmt::Display for TrustDomain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The identity of one service in a trust domain.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct WorkloadIdentity {
    service: ServiceName,
    trust_domain: TrustDomain,
}

impl WorkloadIdentity {
    pub fn new(service: ServiceName, trust_domain: TrustDomain) -> Self {
        Self {
            service,
            trust_domain,
        }
    }

    pub fn service(&self) -> &ServiceName {
        &self.service
    }

    pub fn trust_domain(&self) -> &TrustDomain {
        &self.trust_domain
    }

    /// The DNS identity peers verify, `<service>.<trust domain>`.
    pub fn dns_name(&self) -> String {
        format!("{}.{}", self.service, self.trust_domain)
    }

    /// The SPIFFE ID, `spiffe://<trust domain>/service/<service>`.
    pub fn spiffe_id(&self) -> String {
        format!("spiffe://{}/service/{}", self.trust_domain, self.service)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_name_services_in_their_trust_domain() {
        let identity = WorkloadIdentity::new(
            ServiceName::new("config-service").unwrap(),
            TrustDomain::default(),
        );
        assert_eq!(identity.dns_name(), "config-service.pingora-panel.internal");
        assert_eq!(
            identity.spiffe_id(),
            "spiffe://pingora-panel.internal/service/config-service"
        );
        for invalid in [
            "internal",
            "Panel.internal",
            "-a.internal",
            "a..internal",
            "a_b.internal",
        ] {
            assert!(TrustDomain::new(invalid).is_err(), "{invalid}");
        }
    }
}
