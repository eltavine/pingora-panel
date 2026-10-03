use crate::{
    intake::{accept, Accepted},
    pem::{self, CERTIFICATE},
};
use chrono::{DateTime, Duration, Utc};
use panel_domain::NormalizedHost;
use panel_errors::{PanelError, Result};
use rcgen::{
    string::Ia5String, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose, SanType, SerialNumber, PKCS_ECDSA_P256_SHA256,
};
use std::net::IpAddr;
use time::OffsetDateTime;
use zeroize::Zeroizing;

/// The longest validity a generated certificate may have; clients such as
/// Apple's refuse longer ones.
pub const MAX_SELF_SIGNED_DAYS: u32 = 825;
const MAX_NAMES: usize = 100;
/// Tolerates clocks of clients that run slightly behind.
const BACKDATE: Duration = Duration::minutes(5);

/// Generates an ECDSA P-256 certificate signed by its own key for `names`,
/// which are DNS names, wildcards or IP addresses, valid for `days`.
pub fn self_signed(names: &[String], days: u32, now: DateTime<Utc>) -> Result<Accepted> {
    let names = subject_names(names)?;
    if days == 0 || days > MAX_SELF_SIGNED_DAYS {
        return Err(PanelError::validation_failed(format!(
            "a generated certificate is valid for 1 to {MAX_SELF_SIGNED_DAYS} days"
        )));
    }
    let mut params = CertificateParams::default();
    let mut subject = DistinguishedName::new();
    subject.push(DnType::CommonName, names[0].0.clone());
    params.distinguished_name = subject;
    params.subject_alt_names = names.into_iter().map(|(_, name)| name).collect();
    params.is_ca = IsCa::ExplicitNoCa;
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    params.not_before = timestamp(now - BACKDATE)?;
    params.not_after = timestamp(now + Duration::days(i64::from(days)))?;
    params.serial_number = Some(serial()?);
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)
        .map_err(|error| PanelError::internal(format!("key generation failed: {error}")))?;
    let certificate = params
        .self_signed(&key)
        .map_err(|error| PanelError::internal(format!("certificate generation failed: {error}")))?;
    let key = Zeroizing::new(pem::encode("PRIVATE KEY", &key.serialize_der()));
    accept(&pem::encode(CERTIFICATE, certificate.der()), &key, now)
}

/// The names a certificate is requested for, checked and normalized: DNS
/// names, wildcards or IP addresses, each listed once.
pub fn requested_names(names: &[String]) -> Result<Vec<String>> {
    let checked: Vec<String> = subject_names(names)?
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    for (index, name) in checked.iter().enumerate() {
        if checked[..index].contains(name) {
            return Err(PanelError::validation_failed(format!(
                "{name} is listed twice"
            )));
        }
    }
    Ok(checked)
}

/// Checks the names a certificate is requested for and gives each with its
/// subject alternative name.
pub(crate) fn subject_names(names: &[String]) -> Result<Vec<(String, SanType)>> {
    if names.is_empty() || names.len() > MAX_NAMES {
        return Err(PanelError::validation_failed(format!(
            "a certificate needs 1 to {MAX_NAMES} names"
        )));
    }
    names.iter().map(|name| subject_name(name.trim())).collect()
}

fn subject_name(name: &str) -> Result<(String, SanType)> {
    if let Ok(address) = name.parse::<IpAddr>() {
        return Ok((address.to_string(), SanType::IpAddress(address)));
    }
    let host = NormalizedHost::new(name).map_err(|error| {
        PanelError::validation_failed(format!("{name:?} is not a host name: {error}"))
    })?;
    let ascii = Ia5String::try_from(host.as_str())
        .map_err(|_| PanelError::validation_failed(format!("{name:?} is not a host name")))?;
    Ok((host.as_str().to_owned(), SanType::DnsName(ascii)))
}

fn timestamp(value: DateTime<Utc>) -> Result<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp(value.timestamp())
        .map_err(|_| PanelError::validation_failed("the validity is out of range"))
}

fn serial() -> Result<SerialNumber> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)
        .map_err(|error| PanelError::internal(format!("random generation failed: {error}")))?;
    // RFC 5280 serial numbers are positive integers.
    bytes[0] &= 0x7f;
    Ok(SerialNumber::from_slice(&bytes))
}
