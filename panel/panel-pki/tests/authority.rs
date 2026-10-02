#![forbid(unsafe_code)]

use chrono::{Duration as Span, Utc};
use panel_context::ServiceName;
use panel_pki::{
    CertificateAuthority, CredentialFiles, IssuanceTarget, TrustDomain, DEFAULT_AUTHORITY_VALIDITY,
};
use rustls_pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use std::{path::PathBuf, time::Duration};
use webpki::{anchor_from_trusted_cert, EndEntityCert, KeyUsage};

struct Directory(PathBuf);

impl Directory {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "panel-pki-{name}-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        Self(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn certificate(pem: &str) -> CertificateDer<'static> {
    CertificateDer::from_pem_slice(pem.as_bytes()).unwrap()
}

#[test]
fn the_authority_is_created_once_and_reloaded_for_its_trust_domain_only() {
    let directory = Directory::new("authority");
    let now = Utc::now();
    let (created, fresh) = CertificateAuthority::load_or_create(
        &directory.0,
        TrustDomain::default(),
        DEFAULT_AUTHORITY_VALIDITY,
        now,
    )
    .unwrap();
    assert!(fresh);
    let (loaded, fresh) = CertificateAuthority::load_or_create(
        &directory.0,
        TrustDomain::default(),
        DEFAULT_AUTHORITY_VALIDITY,
        now,
    )
    .unwrap();
    assert!(!fresh);
    assert_eq!(loaded.certificate_pem(), created.certificate_pem());

    let other = TrustDomain::new("other.internal").unwrap();
    assert!(CertificateAuthority::load_or_create(
        &directory.0,
        other,
        DEFAULT_AUTHORITY_VALIDITY,
        now
    )
    .is_err());

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(directory.0.join("authority.key"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    std::fs::remove_file(directory.0.join("authority.crt")).unwrap();
    assert!(CertificateAuthority::load_or_create(
        &directory.0,
        TrustDomain::default(),
        DEFAULT_AUTHORITY_VALIDITY,
        now
    )
    .is_err());
}

#[test]
fn issued_certificates_chain_to_the_authority_for_their_identity_only() {
    let directory = Directory::new("issue");
    let now = Utc::now();
    let (authority, _) = CertificateAuthority::load_or_create(
        &directory.0,
        TrustDomain::default(),
        DEFAULT_AUTHORITY_VALIDITY,
        now,
    )
    .unwrap();
    let issued = authority
        .issue(
            &ServiceName::new("config-service").unwrap(),
            &["config-service".into()],
            Duration::from_secs(24 * 3600),
            now,
        )
        .unwrap();
    assert_eq!(
        issued.not_after - issued.not_before,
        Span::hours(24) + Span::minutes(5)
    );
    assert!(PrivateKeyDer::from_pem_slice(issued.private_key_pem.as_bytes()).is_ok());

    let authority_der = certificate(authority.certificate_pem());
    let anchor = anchor_from_trusted_cert(&authority_der).unwrap();
    let leaf_der = certificate(&issued.certificate_pem);
    let leaf = EndEntityCert::try_from(&leaf_der).unwrap();
    let time =
        UnixTime::since_unix_epoch(Duration::from_secs(u64::try_from(now.timestamp()).unwrap()));
    for usage in [KeyUsage::server_auth(), KeyUsage::client_auth()] {
        leaf.verify_for_usage(
            &[webpki::ring::ECDSA_P256_SHA256],
            std::slice::from_ref(&anchor),
            &[],
            time,
            usage,
            None,
            None,
        )
        .unwrap();
    }
    let name = |value: &str| ServerName::try_from(value.to_owned()).unwrap();
    assert!(leaf
        .verify_is_valid_for_subject_name(&name("config-service.pingora-panel.internal"))
        .is_ok());
    assert!(leaf
        .verify_is_valid_for_subject_name(&name("config-service"))
        .is_ok());
    assert!(leaf
        .verify_is_valid_for_subject_name(&name("panel-api.pingora-panel.internal"))
        .is_err());
    let spiffe = b"spiffe://pingora-panel.internal/service/config-service";
    assert!(leaf_der
        .windows(spiffe.len())
        .any(|window| window == spiffe));

    let expired = UnixTime::since_unix_epoch(Duration::from_secs(
        u64::try_from((now + Span::days(2)).timestamp()).unwrap(),
    ));
    assert!(leaf
        .verify_for_usage(
            &[webpki::ring::ECDSA_P256_SHA256],
            &[anchor],
            &[],
            expired,
            KeyUsage::server_auth(),
            None,
            None,
        )
        .is_err());
}

#[test]
fn credential_files_are_atomic_private_and_renewed_at_two_thirds_of_their_lifetime() {
    let directory = Directory::new("files");
    let now = Utc::now();
    let (authority, _) = CertificateAuthority::load_or_create(
        &directory.0.join("authority"),
        TrustDomain::default(),
        DEFAULT_AUTHORITY_VALIDITY,
        now,
    )
    .unwrap();
    let files = CredentialFiles::new(directory.0.join("panel-api"));
    assert!(
        files.renewal_due(now).unwrap(),
        "missing credentials are due"
    );

    let issued = authority
        .issue(
            &ServiceName::new("panel-api").unwrap(),
            &[],
            Duration::from_secs(3 * 3600),
            now,
        )
        .unwrap();
    files.write(&issued, authority.certificate_pem()).unwrap();
    let validity = files.validity().unwrap().unwrap();
    assert_eq!(validity.not_after.timestamp(), issued.not_after.timestamp());
    assert!(!files.renewal_due(now).unwrap());
    assert!(!files.renewal_due(now + Span::minutes(110)).unwrap());
    assert!(files.renewal_due(now + Span::minutes(120)).unwrap());

    let identity = std::fs::read(files.identity_path()).unwrap();
    assert!(PrivateKeyDer::from_pem_slice(&identity).is_ok());
    assert_eq!(CertificateDer::pem_slice_iter(&identity).count(), 1);
    let trust = std::fs::read(files.trust_path()).unwrap();
    assert_eq!(CertificateDer::pem_slice_iter(&trust).count(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(files.identity_path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[test]
fn renewal_issues_only_missing_or_due_credentials() {
    let directory = Directory::new("renew");
    let now = Utc::now();
    let (authority, _) = CertificateAuthority::load_or_create(
        &directory.0.join("authority"),
        TrustDomain::default(),
        DEFAULT_AUTHORITY_VALIDITY,
        now,
    )
    .unwrap();
    let targets: Vec<IssuanceTarget> = ["panel-api", "config-service"]
        .into_iter()
        .map(|service| IssuanceTarget {
            service: ServiceName::new(service).unwrap(),
            files: CredentialFiles::new(directory.0.join(service)),
            alternative_names: vec![service.to_owned()],
        })
        .collect();
    let lifetime = Duration::from_secs(3 * 3600);
    assert_eq!(
        authority.renew_due(&targets, lifetime, now).unwrap().len(),
        2
    );
    assert!(authority
        .renew_due(&targets, lifetime, now)
        .unwrap()
        .is_empty());
    assert_eq!(
        authority
            .renew_due(&targets, lifetime, now + Span::hours(2))
            .unwrap()
            .len(),
        2
    );
}
