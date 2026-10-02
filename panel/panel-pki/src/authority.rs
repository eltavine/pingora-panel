use crate::{files::write_atomic, pem, TrustDomain, WorkloadIdentity};
use chrono::{DateTime, Utc};
use panel_context::ServiceName;
use panel_errors::{PanelError, Result};
use rcgen::{
    string::Ia5String, BasicConstraints, CertificateParams, DistinguishedName, DnType,
    ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose, SanType, SerialNumber,
};
use rustls_pki_types::{pem::PemObject, PrivateKeyDer};
use std::{path::Path, time::Duration};
use time::OffsetDateTime;

/// How long a new authority's certificate is valid.
pub const DEFAULT_AUTHORITY_VALIDITY: Duration = Duration::from_secs(10 * 365 * 24 * 3600);
const AUTHORITY_KEY: &str = "authority.key";
const AUTHORITY_CERTIFICATE: &str = "authority.crt";
/// Certificates start this long before issuance to tolerate clock skew.
const BACKDATE: Duration = Duration::from_secs(300);

/// A service's newly issued key and certificate.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssuedCredentials {
    pub certificate_pem: String,
    pub private_key_pem: String,
    pub serial: String,
    pub not_before: DateTime<Utc>,
    pub not_after: DateTime<Utc>,
}

/// The installation's certificate authority.
pub struct CertificateAuthority {
    trust_domain: TrustDomain,
    issuer: Issuer<'static, KeyPair>,
    certificate_pem: String,
}

fn failure(context: &str) -> impl FnOnce(rcgen::Error) -> PanelError + '_ {
    move |error| PanelError::internal(format!("{context}: {error}"))
}

fn timestamp(value: DateTime<Utc>) -> Result<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp(value.timestamp())
        .map_err(|_| PanelError::invalid_argument("time is out of range"))
}

fn span(value: Duration) -> Result<chrono::Duration> {
    chrono::Duration::from_std(value)
        .map_err(|_| PanelError::invalid_argument("duration is too long"))
}

fn common_name(trust_domain: &TrustDomain) -> String {
    format!("Pingora Panel internal CA ({trust_domain})")
}

/// The parameters that identify the authority; issuance rebuilds them, so
/// they must not change once certificates exist.
fn authority_params(trust_domain: &TrustDomain) -> CertificateParams {
    let mut params = CertificateParams::default();
    let mut name = DistinguishedName::new();
    name.push(DnType::OrganizationName, "Pingora Panel");
    name.push(DnType::CommonName, common_name(trust_domain));
    params.distinguished_name = name;
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    params
}

fn serial() -> Result<SerialNumber> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|error| PanelError::internal(format!("random generation failed: {error}")))?;
    // RFC 5280 serial numbers are positive integers.
    bytes[0] &= 0x7f;
    Ok(SerialNumber::from_slice(&bytes))
}

impl CertificateAuthority {
    /// Loads the authority kept in `directory`, creating it on first use.
    /// Returns whether it was created.
    pub fn load_or_create(
        directory: &Path,
        trust_domain: TrustDomain,
        validity: Duration,
        now: DateTime<Utc>,
    ) -> Result<(Self, bool)> {
        let key_path = directory.join(AUTHORITY_KEY);
        let certificate_path = directory.join(AUTHORITY_CERTIFICATE);
        match (key_path.exists(), certificate_path.exists()) {
            (true, true) => {
                let read = |path: &Path| {
                    std::fs::read_to_string(path).map_err(|error| {
                        PanelError::storage_unavailable(format!(
                            "cannot read {}: {error}",
                            path.display()
                        ))
                    })
                };
                let key_pem = read(&key_path)?;
                let certificate_pem = read(&certificate_path)?;
                let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes())
                    .map_err(|_| PanelError::corrupt_state("the authority key is not PEM"))?;
                let key =
                    KeyPair::try_from(&key).map_err(failure("the authority key is invalid"))?;
                if !certificate_pem.contains("BEGIN CERTIFICATE")
                    || !rustls_pki_types::CertificateDer::from_pem_slice(certificate_pem.as_bytes())
                        .map(|der| {
                            der.windows(common_name(&trust_domain).len())
                                .any(|window| window == common_name(&trust_domain).as_bytes())
                        })
                        .unwrap_or(false)
                {
                    return Err(PanelError::precondition_failed(format!(
                        "the authority in {} does not belong to trust domain {trust_domain}",
                        directory.display()
                    )));
                }
                Ok((
                    Self {
                        issuer: Issuer::new(authority_params(&trust_domain), key),
                        trust_domain,
                        certificate_pem,
                    },
                    false,
                ))
            }
            (false, false) => {
                std::fs::create_dir_all(directory).map_err(|error| {
                    PanelError::storage_unavailable(format!(
                        "cannot create {}: {error}",
                        directory.display()
                    ))
                })?;
                let key = KeyPair::generate().map_err(failure("key generation failed"))?;
                let mut params = authority_params(&trust_domain);
                params.not_before = timestamp(now - span(BACKDATE)?)?;
                params.not_after = timestamp(now + span(validity)?)?;
                params.serial_number = Some(serial()?);
                let certificate = params
                    .self_signed(&key)
                    .map_err(failure("authority certificate generation failed"))?;
                let certificate_pem = pem::encode("CERTIFICATE", certificate.der());
                write_atomic(
                    &key_path,
                    &pem::encode("PRIVATE KEY", &key.serialize_der()),
                    true,
                )?;
                write_atomic(&certificate_path, &certificate_pem, false)?;
                Ok((
                    Self {
                        issuer: Issuer::new(authority_params(&trust_domain), key),
                        trust_domain,
                        certificate_pem,
                    },
                    true,
                ))
            }
            _ => Err(PanelError::corrupt_state(format!(
                "{} must hold both the authority key and certificate, or neither",
                directory.display()
            ))),
        }
    }

    pub fn trust_domain(&self) -> &TrustDomain {
        &self.trust_domain
    }

    /// The authority certificate services trust.
    pub fn certificate_pem(&self) -> &str {
        &self.certificate_pem
    }

    /// Issues a fresh key and certificate for `service`, valid for
    /// `validity` and additionally for `alternative_names`, such as the
    /// host name peers connect to.
    pub fn issue(
        &self,
        service: &ServiceName,
        alternative_names: &[String],
        validity: Duration,
        now: DateTime<Utc>,
    ) -> Result<IssuedCredentials> {
        let identity = WorkloadIdentity::new(service.clone(), self.trust_domain.clone());
        let mut names = vec![identity.dns_name()];
        names.extend(alternative_names.iter().cloned());
        let mut params =
            CertificateParams::new(names).map_err(failure("invalid alternative names"))?;
        params.subject_alt_names.push(SanType::URI(
            Ia5String::try_from(identity.spiffe_id()).map_err(failure("invalid SPIFFE ID"))?,
        ));
        params
            .distinguished_name
            .push(DnType::CommonName, identity.dns_name());
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![
            ExtendedKeyUsagePurpose::ServerAuth,
            ExtendedKeyUsagePurpose::ClientAuth,
        ];
        params.use_authority_key_identifier_extension = true;
        let not_before = now - span(BACKDATE)?;
        let not_after = now + span(validity)?;
        params.not_before = timestamp(not_before)?;
        params.not_after = timestamp(not_after)?;
        let serial = serial()?;
        params.serial_number = Some(serial.clone());
        let key = KeyPair::generate().map_err(failure("key generation failed"))?;
        let certificate = params
            .signed_by(&key, &self.issuer)
            .map_err(failure("certificate issuance failed"))?;
        Ok(IssuedCredentials {
            certificate_pem: pem::encode("CERTIFICATE", certificate.der()),
            private_key_pem: pem::encode("PRIVATE KEY", &key.serialize_der()),
            serial: serial
                .as_ref()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            not_before: DateTime::from_timestamp(not_before.timestamp(), 0).unwrap_or(not_before),
            not_after: DateTime::from_timestamp(not_after.timestamp(), 0).unwrap_or(not_after),
        })
    }
}
