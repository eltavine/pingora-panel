//! Downstream server certificates chosen by SNI from the active snapshot.

use crate::{adapter::ActiveSnapshot, secrets::SecretSource};
use async_trait::async_trait;
use panel_errors::{PanelError, Result};
use panel_ir::{RuntimeSnapshot, TlsProfile};
use pingora_core::{listeners::TlsAccept, protocols::tls::TlsRef};
use rustls::{
    server::{ClientHello, ResolvesServerCert},
    sign::CertifiedKey,
};
use rustls_pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use sha2::{Digest, Sha256};
use std::{any::Any, collections::HashMap, fmt, sync::Arc};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum TlsVersion {
    Tls12,
    Tls13,
}

impl TlsVersion {
    /// Accepts `TLSv1.3`, `TLS1.3` and the `TLSv1_3` form rustls reports.
    fn parse(value: &str) -> Option<Self> {
        let normalized: String = value
            .to_ascii_lowercase()
            .chars()
            .filter(|character| *character != 'v')
            .map(|character| if character == '_' { '.' } else { character })
            .collect();
        match normalized.as_str() {
            "tls1.2" => Some(Self::Tls12),
            "tls1.3" => Some(Self::Tls13),
            _ => None,
        }
    }
}

pub(crate) struct ServerCertificate {
    key: Arc<CertifiedKey>,
    pub minimum: TlsVersion,
}

#[derive(Default)]
pub(crate) struct CertificateIndex {
    profiles: HashMap<String, Arc<ServerCertificate>>,
    exact: HashMap<String, Arc<ServerCertificate>>,
    /// Keyed by the parent of `*.parent`.
    wildcard: HashMap<String, Arc<ServerCertificate>>,
    listeners: HashMap<String, Arc<ServerCertificate>>,
    /// The digest of the files the index was built from.
    material: [u8; 32],
}

impl CertificateIndex {
    /// SHA-256 over the chain and key files `snapshot`'s TLS profiles name,
    /// to notice when they change on disk.
    pub(crate) fn material(
        snapshot: &RuntimeSnapshot,
        secrets: &dyn SecretSource,
    ) -> Result<[u8; 32]> {
        let mut digest = Sha256::new();
        for profile in &snapshot.tls_profiles {
            for id in [
                &profile.certificate_secret_id,
                &profile.private_key_secret_id,
            ] {
                let content = secrets.read(id)?;
                digest.update((content.len() as u64).to_be_bytes());
                digest.update(&content);
            }
        }
        Ok(digest.finalize().into())
    }

    pub(crate) fn built_from(&self, material: &[u8; 32]) -> bool {
        self.material == *material
    }

    pub(crate) fn build(snapshot: &RuntimeSnapshot, secrets: &dyn SecretSource) -> Result<Self> {
        let mut index = Self {
            material: Self::material(snapshot, secrets)?,
            ..Self::default()
        };
        for profile in &snapshot.tls_profiles {
            index.profiles.insert(
                profile.id.clone(),
                Arc::new(load_profile(profile, secrets)?),
            );
        }
        let profile = |id: &str| {
            index.profiles.get(id).cloned().ok_or_else(|| {
                PanelError::validation_failed(format!("TLS profile {id} does not exist"))
            })
        };
        let mut exact = HashMap::new();
        let mut wildcard = HashMap::new();
        for site in snapshot.sites.iter().filter(|site| site.enabled) {
            for domain in site.domains.iter().filter(|domain| domain.enabled) {
                let Some(id) = &domain.tls_profile_id else {
                    continue;
                };
                let certificate = profile(id)?;
                match domain.host.as_str().strip_prefix("*.") {
                    Some(parent) => wildcard.insert(parent.to_owned(), certificate),
                    None => exact.insert(domain.host.as_str().to_owned(), certificate),
                };
            }
        }
        let mut listeners = HashMap::new();
        for listener in &snapshot.listeners {
            if let Some(id) = &listener.tls_profile_id {
                listeners.insert(listener.id.clone(), profile(id)?);
            }
        }
        index.exact = exact;
        index.wildcard = wildcard;
        index.listeners = listeners;
        Ok(index)
    }

    /// The certificate presented for `server_name` on `listener`: the one a
    /// domain claims, else the listener's own profile.
    pub(crate) fn presented(
        &self,
        listener: &str,
        server_name: Option<&str>,
    ) -> Option<&Arc<ServerCertificate>> {
        server_name
            .and_then(|name| self.claimed(name))
            .or_else(|| self.listeners.get(listener))
    }

    fn claimed(&self, name: &str) -> Option<&Arc<ServerCertificate>> {
        self.exact.get(name).or_else(|| {
            name.split_once('.')
                .and_then(|(_, parent)| self.wildcard.get(parent))
        })
    }

    /// Whether a connection established for `server_name` may carry requests
    /// for `host` (RFC 9110 §7.4): both resolve to the same certificate.
    pub(crate) fn covers(&self, listener: &str, server_name: &str, host: &str) -> bool {
        match (
            self.presented(listener, Some(server_name)),
            self.presented(listener, Some(host)),
        ) {
            (Some(left), Some(right)) => Arc::ptr_eq(left, right),
            _ => false,
        }
    }
}

fn load_profile(profile: &TlsProfile, secrets: &dyn SecretSource) -> Result<ServerCertificate> {
    let invalid = |detail: String| {
        PanelError::validation_failed(format!("TLS profile {}: {detail}", profile.id))
    };
    let minimum = TlsVersion::parse(&profile.min_protocol).ok_or_else(|| {
        invalid(format!(
            "unsupported minimum protocol {}",
            profile.min_protocol
        ))
    })?;
    if let Some(protocol) = profile
        .alpn
        .iter()
        .find(|protocol| !matches!(protocol.as_str(), "h2" | "http/1.1"))
    {
        return Err(invalid(format!("unsupported ALPN protocol {protocol}")));
    }
    let chain = CertificateDer::pem_slice_iter(&secrets.read(&profile.certificate_secret_id)?)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| invalid(format!("certificate chain is not PEM: {error}")))?;
    if chain.is_empty() {
        return Err(invalid("certificate chain is empty".into()));
    }
    let key = PrivateKeyDer::from_pem_slice(&secrets.read(&profile.private_key_secret_id)?)
        .map_err(|error| invalid(format!("private key is not PEM: {error}")))?;
    let signing_key = rustls::crypto::ring::sign::any_supported_type(&key)
        .map_err(|error| invalid(format!("unsupported private key: {error}")))?;
    let key = CertifiedKey::new(chain, signing_key);
    key.keys_match().map_err(|error| {
        invalid(format!(
            "private key does not match the certificate: {error}"
        ))
    })?;
    Ok(ServerCertificate {
        key: Arc::new(key),
        minimum,
    })
}

/// Serves the certificate the active snapshot assigns to each handshake.
pub(crate) struct ListenerCertificates {
    active: ActiveSnapshot,
    listener: String,
}

impl ListenerCertificates {
    pub(crate) fn new(active: ActiveSnapshot, listener: String) -> Self {
        Self { active, listener }
    }
}

impl fmt::Debug for ListenerCertificates {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ListenerCertificates")
            .field("listener", &self.listener)
            .finish_non_exhaustive()
    }
}

impl ResolvesServerCert for ListenerCertificates {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let active = self.active.load();
        let name = hello.server_name().map(str::to_ascii_lowercase);
        let certificates = active.as_ref()?.certificates.load();
        certificates
            .presented(&self.listener, name.as_deref())
            .map(|certificate| Arc::clone(&certificate.key))
    }
}

/// What the TLS handshake negotiated, attached to the connection digest.
#[derive(Debug)]
pub(crate) struct Handshake {
    pub server_name: Option<String>,
    pub version: Option<TlsVersion>,
}

pub(crate) struct HandshakeRecorder;

#[async_trait]
impl TlsAccept for HandshakeRecorder {
    async fn handshake_complete_callback(
        &self,
        tls: &TlsRef,
    ) -> Option<Arc<dyn Any + Send + Sync>> {
        Some(Arc::new(Handshake {
            server_name: tls.server_name().map(str::to_ascii_lowercase),
            version: tls.version().and_then(TlsVersion::parse),
        }))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use base64::Engine;
    use panel_domain::{NormalizedHost, RevisionId, SiteId};
    use panel_ir::{DomainSpec, ListenerRef, SiteSpec};
    use std::collections::BTreeSet;

    pub(crate) struct Secrets(pub HashMap<String, Vec<u8>>);

    impl SecretSource for Secrets {
        fn read(&self, id: &str) -> Result<Vec<u8>> {
            self.0
                .get(id)
                .cloned()
                .ok_or_else(|| PanelError::validation_failed(format!("missing {id}")))
        }
    }

    pub(crate) fn pem(label: &str, der: &[u8]) -> Vec<u8> {
        let encoded = base64::engine::general_purpose::STANDARD.encode(der);
        let mut output = format!("-----BEGIN {label}-----\n");
        for line in encoded.as_bytes().chunks(64) {
            output.push_str(std::str::from_utf8(line).unwrap());
            output.push('\n');
        }
        output.push_str(&format!("-----END {label}-----\n"));
        output.into_bytes()
    }

    /// Self-signed certificate and PKCS #8 key for `names`, as PEM.
    pub(crate) fn certificate(names: &[&str]) -> (Vec<u8>, Vec<u8>) {
        let certified = rcgen::generate_simple_self_signed(
            names
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>(),
        )
        .unwrap();
        (
            pem("CERTIFICATE", certified.cert.der()),
            pem("PRIVATE KEY", &certified.signing_key.serialize_der()),
        )
    }

    pub(crate) fn profile(id: &str) -> TlsProfile {
        TlsProfile {
            id: id.into(),
            certificate_secret_id: format!("{id}.crt"),
            private_key_secret_id: format!("{id}.key"),
            min_protocol: "TLSv1.2".into(),
            alpn: BTreeSet::new(),
        }
    }

    pub(crate) fn secrets(profiles: &[(&str, &[&str])]) -> Secrets {
        let mut material = HashMap::new();
        for (id, names) in profiles {
            let (chain, key) = certificate(names);
            material.insert(format!("{id}.crt"), chain);
            material.insert(format!("{id}.key"), key);
        }
        Secrets(material)
    }

    #[test]
    fn domains_claim_certificates_and_listeners_provide_fallbacks() {
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        snapshot.tls_profiles = vec![profile("main"), profile("wild"), profile("default")];
        let mut listener = ListenerRef::new("https", "0.0.0.0:443");
        listener.tls_profile_id = Some("default".into());
        snapshot.listeners.push(listener);
        let mut exact = DomainSpec::new(NormalizedHost::new("example.com").unwrap());
        exact.tls_profile_id = Some("main".into());
        let mut wild = DomainSpec::new(NormalizedHost::new("*.example.com").unwrap());
        wild.tls_profile_id = Some("wild".into());
        snapshot.sites.push(SiteSpec::new(
            SiteId::new("site").unwrap(),
            "site",
            vec![exact, wild],
        ));
        let index = CertificateIndex::build(
            &snapshot,
            &secrets(&[
                ("main", &["example.com"]),
                ("wild", &["*.example.com"]),
                ("default", &["localhost"]),
            ]),
        )
        .unwrap();
        let presented = |name| index.presented("https", name).map(Arc::as_ptr);
        assert_eq!(
            presented(Some("example.com")),
            Some(Arc::as_ptr(&index.profiles["main"]))
        );
        assert_eq!(
            presented(Some("api.example.com")),
            Some(Arc::as_ptr(&index.profiles["wild"]))
        );
        assert_eq!(
            presented(Some("unknown.test")),
            Some(Arc::as_ptr(&index.profiles["default"]))
        );
        assert_eq!(
            presented(None),
            Some(Arc::as_ptr(&index.profiles["default"]))
        );
        assert!(index.covers("https", "a.example.com", "b.example.com"));
        assert!(!index.covers("https", "example.com", "a.example.com"));
    }

    #[test]
    fn mismatched_or_invalid_material_is_rejected() {
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        snapshot.tls_profiles.push(profile("main"));
        let mut secrets = secrets(&[("main", &["example.com"]), ("other", &["other.test"])]);
        let other_key = secrets.0["other.key"].clone();
        secrets.0.insert("main.key".into(), other_key);
        let error = CertificateIndex::build(&snapshot, &secrets).err().unwrap();
        assert!(error.message.contains("does not match"), "{error}");
        secrets.0.insert("main.crt".into(), b"garbage".to_vec());
        assert!(CertificateIndex::build(&snapshot, &secrets).is_err());
        snapshot.tls_profiles[0].min_protocol = "SSLv3".into();
        assert!(CertificateIndex::build(&snapshot, &secrets).is_err());
    }

    #[test]
    fn protocol_names_parse_in_both_spellings() {
        assert_eq!(TlsVersion::parse("TLSv1.3"), Some(TlsVersion::Tls13));
        assert_eq!(TlsVersion::parse("TLSv1_2"), Some(TlsVersion::Tls12));
        assert_eq!(TlsVersion::parse("TLS1.2"), Some(TlsVersion::Tls12));
        assert_eq!(TlsVersion::parse("TLSv1.1"), None);
    }
}
