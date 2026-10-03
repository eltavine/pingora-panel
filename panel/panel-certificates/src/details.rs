use chrono::{DateTime, Duration, Utc};
use panel_domain::NormalizedHost;
use serde::{Deserialize, Serialize};

/// Certificates closer than this to their end count as expiring.
pub const EXPIRING_WITHIN: Duration = Duration::days(30);

/// The kind of key a certificate certifies.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum KeyAlgorithm {
    Rsa,
    EcdsaP256,
    EcdsaP384,
    Ed25519,
}

/// What a certificate chain says about itself.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CertificateDetails {
    /// The leaf's subject as an RFC 4514 string.
    pub subject: String,
    pub issuer: String,
    /// The serial number in hexadecimal.
    pub serial: String,
    /// DNS names and IP addresses from the subject alternative names.
    pub names: Vec<String>,
    pub not_before: DateTime<Utc>,
    pub not_after: DateTime<Utc>,
    /// SHA-256 of the leaf certificate in hexadecimal.
    pub fingerprint: String,
    /// SHA-256 of the leaf's public key information in hexadecimal.
    pub public_key_fingerprint: String,
    pub key_algorithm: KeyAlgorithm,
    pub key_bits: u32,
    /// Certificates in the chain, the leaf included.
    pub chain_length: u32,
    /// Whether the leaf names itself as its issuer.
    pub self_signed: bool,
}

/// Where a certificate stands in its validity period.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum CertificateStatus {
    NotYetValid,
    Valid,
    /// Ends within [`EXPIRING_WITHIN`].
    Expiring,
    Expired,
}

impl CertificateDetails {
    /// Whether a subject alternative name matches `host` (RFC 9525 §6.3):
    /// a wildcard name covers exactly one more left-most label, and a
    /// wildcard host is covered only by the same wildcard.
    pub fn covers(&self, host: &NormalizedHost) -> bool {
        self.names
            .iter()
            .filter_map(|name| NormalizedHost::new(name).ok())
            .any(|name| name == *host || name.matches(host))
    }

    /// The hosts among `hosts` that no name of the certificate covers.
    pub fn uncovered<'a>(
        &self,
        hosts: impl IntoIterator<Item = &'a NormalizedHost>,
    ) -> Vec<&'a NormalizedHost> {
        hosts
            .into_iter()
            .filter(|host| !self.covers(host))
            .collect()
    }

    pub fn status(&self, now: DateTime<Utc>) -> CertificateStatus {
        if now < self.not_before {
            CertificateStatus::NotYetValid
        } else if now >= self.not_after {
            CertificateStatus::Expired
        } else if self.not_after - now <= EXPIRING_WITHIN {
            CertificateStatus::Expiring
        } else {
            CertificateStatus::Valid
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn details(names: &[&str]) -> CertificateDetails {
        CertificateDetails {
            subject: "CN=example.com".into(),
            issuer: "CN=Example CA".into(),
            serial: "01".into(),
            names: names.iter().map(|name| (*name).into()).collect(),
            not_before: Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            not_after: Utc.with_ymd_and_hms(2026, 4, 1, 0, 0, 0).unwrap(),
            fingerprint: String::new(),
            public_key_fingerprint: String::new(),
            key_algorithm: KeyAlgorithm::EcdsaP256,
            key_bits: 256,
            chain_length: 1,
            self_signed: false,
        }
    }

    fn host(name: &str) -> NormalizedHost {
        NormalizedHost::new(name).unwrap()
    }

    #[test]
    fn names_cover_hosts_as_rfc_9525_matches_them() {
        let certificate = details(&[
            "Example.COM",
            "*.example.com",
            "xn--bcher-kva.example",
            "10.0.0.1",
        ]);

        for covered in [
            "example.com",
            "www.example.com",
            "API.example.com",
            "bücher.example",
            "*.example.com",
        ] {
            assert!(certificate.covers(&host(covered)), "{covered}");
        }
        for uncovered in [
            "a.b.example.com",
            "example.org",
            "*.www.example.com",
            "bucher.example",
        ] {
            assert!(!certificate.covers(&host(uncovered)), "{uncovered}");
        }
        let hosts = [host("www.example.com"), host("example.net")];
        assert_eq!(certificate.uncovered(&hosts), [&hosts[1]]);
    }

    #[test]
    fn wildcard_hosts_need_the_same_wildcard() {
        let certificate = details(&["www.example.com"]);
        assert!(!certificate.covers(&host("*.example.com")));
        let partial = details(&["w*.example.com", "*"]);
        assert!(!partial.covers(&host("www.example.com")));
    }

    #[test]
    fn status_follows_the_validity_period() {
        let certificate = details(&["example.com"]);
        let at = |month, day| Utc.with_ymd_and_hms(2026, month, day, 0, 0, 0).unwrap();
        assert_eq!(
            certificate.status(at(1, 1) - Duration::seconds(1)),
            CertificateStatus::NotYetValid
        );
        assert_eq!(certificate.status(at(1, 15)), CertificateStatus::Valid);
        assert_eq!(certificate.status(at(3, 2)), CertificateStatus::Expiring);
        assert_eq!(certificate.status(at(4, 1)), CertificateStatus::Expired);
    }
}
