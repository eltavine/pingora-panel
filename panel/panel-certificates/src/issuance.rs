//! What a CA needs to issue a certificate, and when to replace one.

use crate::{details::CertificateDetails, generate::subject_names, intake::parse_chain, pem};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};
use panel_errors::{PanelError, Result};
use rcgen::{CertificateParams, DistinguishedName, KeyPair, PKCS_ECDSA_P256_SHA256};
use x509_parser::{certificate::X509Certificate, extensions::ParsedExtension, prelude::FromDer};
use zeroize::Zeroizing;

/// A new private key and a PKCS#10 request for a certificate, signed by it.
pub struct SigningRequest {
    /// The DER request an ACME order is finalized with (RFC 8555 §7.4).
    pub der: Vec<u8>,
    /// The key in PKCS#8 PEM.
    pub key: Zeroizing<String>,
}

impl std::fmt::Debug for SigningRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SigningRequest")
            .finish_non_exhaustive()
    }
}

/// A new ECDSA P-256 key and a request for `names`, which are DNS names,
/// wildcards or IP addresses.
pub fn signing_request(names: &[String]) -> Result<SigningRequest> {
    let mut params = CertificateParams::default();
    params.distinguished_name = DistinguishedName::new();
    params.subject_alt_names = subject_names(names)?
        .into_iter()
        .map(|(_, name)| name)
        .collect();
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
        .map_err(|error| PanelError::internal(format!("key generation failed: {error}")))?;
    let request = params.serialize_request(&key).map_err(|error| {
        PanelError::internal(format!("signing request generation failed: {error}"))
    })?;
    Ok(SigningRequest {
        der: request.der().to_vec(),
        key: Zeroizing::new(pem::encode("PRIVATE KEY", &key.serialize_der())),
    })
}

/// The identifier ACME renewal information knows the leaf of `chain` by
/// (RFC 9773 §4.1), when the leaf names its issuer's key.
pub fn renewal_identifier(chain: &str) -> Result<Option<String>> {
    let certificates = parse_chain(chain)?;
    let Ok(([], leaf)) = X509Certificate::from_der(&certificates[0]) else {
        return Err(PanelError::validation_failed(
            "the leaf is not a DER X.509 certificate",
        ));
    };
    let key_identifier =
        leaf.extensions()
            .iter()
            .find_map(|extension| match extension.parsed_extension() {
                ParsedExtension::AuthorityKeyIdentifier(authority) => {
                    authority.key_identifier.as_ref().map(|id| id.0)
                }
                _ => None,
            });
    Ok(key_identifier.map(|id| {
        format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(id),
            URL_SAFE_NO_PAD.encode(leaf.raw_serial())
        )
    }))
}

/// When a certificate is due for replacement unless its CA suggests a
/// window: once a third of its validity remains.
pub fn renewal_time(details: &CertificateDetails) -> DateTime<Utc> {
    details.not_after - (details.not_after - details.not_before) / 3
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{accept, details::KeyAlgorithm, pem::CERTIFICATE};
    use chrono::TimeZone;
    use rcgen::{BasicConstraints, IsCa, Issuer, KeyIdMethod, SanType, SerialNumber};
    use x509_parser::certification_request::X509CertificationRequest;

    #[test]
    fn requests_name_every_host_and_come_with_a_fresh_key() {
        let names = vec!["shop.example".to_owned(), "*.shop.example".to_owned()];
        let request = signing_request(&names).unwrap();
        let (rest, parsed) = X509CertificationRequest::from_der(&request.der).unwrap();
        assert!(rest.is_empty());
        let requested: Vec<String> = parsed
            .requested_extensions()
            .into_iter()
            .flatten()
            .filter_map(|extension| match extension {
                ParsedExtension::SubjectAlternativeName(names) => Some(
                    names
                        .general_names
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>(),
                ),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(
            requested,
            ["DNSName(shop.example)", "DNSName(*.shop.example)"]
        );
        assert!(request.key.starts_with("-----BEGIN PRIVATE KEY-----"));
        assert_ne!(signing_request(&names).unwrap().key, request.key);
        assert!(signing_request(&[]).is_err());
        assert!(signing_request(&["not a host".to_owned()]).is_err());
    }

    #[test]
    fn renewal_identifiers_follow_rfc_9773() {
        // The example of RFC 9773 §4.1.
        let mut authority = CertificateParams::new(Vec::<String>::new()).unwrap();
        authority.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        authority.key_identifier_method = KeyIdMethod::PreSpecified(vec![
            0x69, 0x88, 0x5B, 0x6B, 0x87, 0x46, 0x40, 0x41, 0xE1, 0xB3, 0x7B, 0x84, 0x7B, 0xA0,
            0xAE, 0x2C, 0xDE, 0x01, 0xC8, 0xD4,
        ]);
        let authority_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let authority_certificate = authority.self_signed(&authority_key).unwrap();
        let issuer = Issuer::new(authority, authority_key);

        let mut leaf = CertificateParams::new(vec!["shop.example".to_owned()]).unwrap();
        leaf.serial_number = Some(SerialNumber::from_slice(&[0x87, 0x65, 0x43, 0x21]));
        leaf.use_authority_key_identifier_extension = true;
        leaf.subject_alt_names = vec![SanType::DnsName("shop.example".try_into().unwrap())];
        let leaf_key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
        let leaf_certificate = leaf.signed_by(&leaf_key, &issuer).unwrap();
        let chain = format!(
            "{}{}",
            pem::encode(CERTIFICATE, leaf_certificate.der()),
            pem::encode(CERTIFICATE, authority_certificate.der())
        );
        assert_eq!(
            renewal_identifier(&chain).unwrap().as_deref(),
            Some("aYhba4dGQEHhs3uEe6CuLN4ByNQ.AIdlQyE")
        );

        let own = crate::self_signed(&["shop.example".to_owned()], 30, Utc::now()).unwrap();
        assert_eq!(renewal_identifier(&own.chain).unwrap(), None);
        let accepted = accept(&own.chain, &own.key, Utc::now()).unwrap();
        assert_eq!(accepted.details.key_algorithm, KeyAlgorithm::EcdsaP256);
    }

    #[test]
    fn certificates_renew_when_a_third_of_their_validity_remains() {
        let own = crate::self_signed(&["shop.example".to_owned()], 90, Utc::now()).unwrap();
        let mut details = own.details;
        details.not_before = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        details.not_after = Utc.with_ymd_and_hms(2026, 4, 1, 0, 0, 0).unwrap();
        assert_eq!(
            renewal_time(&details),
            Utc.with_ymd_and_hms(2026, 3, 2, 0, 0, 0).unwrap()
        );
    }
}
