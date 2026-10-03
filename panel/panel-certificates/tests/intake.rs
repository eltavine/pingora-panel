#![forbid(unsafe_code)]

use chrono::{DateTime, Duration, Utc};
use panel_certificates::{
    accept, describe, describe_der, self_signed, CertificateStatus, KeyAlgorithm,
    MAX_SELF_SIGNED_DAYS,
};
use panel_domain::NormalizedHost;
use rcgen::{
    BasicConstraints, CertificateParams, DnType, IsCa, Issuer, KeyPair, SignatureAlgorithm,
    PKCS_ECDSA_P256_SHA256, PKCS_ECDSA_P384_SHA384, PKCS_ED25519,
};
use rustls_pki_types::{pem::PemObject, CertificateDer};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

const RSA_CHAIN: &str = include_str!("fixtures/rsa-2048.pem");
const RSA_KEY: &str = include_str!("fixtures/rsa-2048.key");
const EC_CHAIN: &str = include_str!("fixtures/ec-p256.pem");
const EC_KEY: &str = include_str!("fixtures/ec-p256.key");
const WEAK_CHAIN: &str = include_str!("fixtures/rsa-1024.pem");

fn host(name: &str) -> NormalizedHost {
    NormalizedHost::new(name).unwrap()
}

fn pem(label: &str, der: &[u8]) -> String {
    ::pem::encode(&::pem::Pem::new(label, der))
}

fn timestamp(value: DateTime<Utc>) -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(value.timestamp()).unwrap()
}

struct Authority {
    issuer: Issuer<'static, KeyPair>,
    pem: String,
}

fn authority(name: &str, parent: Option<&Authority>) -> Authority {
    let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).unwrap();
    let mut params = CertificateParams::new(Vec::new()).unwrap();
    params.distinguished_name.push(DnType::CommonName, name);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let certificate = match parent {
        Some(parent) => params.signed_by(&key, &parent.issuer),
        None => params.self_signed(&key),
    }
    .unwrap();
    Authority {
        pem: pem("CERTIFICATE", certificate.der()),
        issuer: Issuer::new(params, key),
    }
}

/// A leaf for `names` and its PKCS#8 key.
fn leaf(
    names: &[&str],
    algorithm: &'static SignatureAlgorithm,
    issuer: Option<&Authority>,
    not_after: DateTime<Utc>,
) -> (String, String) {
    let key = KeyPair::generate_for(algorithm).unwrap();
    let mut params = CertificateParams::new(
        names
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    params.distinguished_name.push(DnType::CommonName, "leaf");
    params.not_before = timestamp(not_after - Duration::days(90));
    params.not_after = timestamp(not_after);
    let certificate = match issuer {
        Some(issuer) => params.signed_by(&key, &issuer.issuer),
        None => params.self_signed(&key),
    }
    .unwrap();
    (
        pem("CERTIFICATE", certificate.der()),
        pem("PRIVATE KEY", &key.serialize_der()),
    )
}

fn refusal(chain: &str, key: &str) -> String {
    accept(chain, key, Utc::now()).unwrap_err().message
}

#[test]
fn pkcs1_rsa_keys_are_accepted_and_described() {
    let accepted = accept(RSA_CHAIN, RSA_KEY, Utc::now()).unwrap();
    let details = &accepted.details;

    assert_eq!(details.key_algorithm, KeyAlgorithm::Rsa);
    assert_eq!(details.key_bits, 2048);
    assert_eq!(details.names, ["rsa.example"]);
    assert_eq!(details.subject, "CN=rsa.example");
    assert!(details.self_signed);
    assert_eq!(details.chain_length, 1);
    let leaf = CertificateDer::from_pem_slice(RSA_CHAIN.as_bytes()).unwrap();
    assert_eq!(details.fingerprint, hex::encode(Sha256::digest(&leaf)));
    assert_eq!(details.fingerprint.len(), 64);
    assert!(!details.serial.is_empty() && !details.serial.starts_with("00"));
    assert!(accepted
        .key
        .starts_with("-----BEGIN RSA PRIVATE KEY-----\n"));
    assert_eq!(
        CertificateDer::from_pem_slice(accepted.chain.as_bytes()).unwrap(),
        leaf
    );
    assert!(!format!("{accepted:?}").contains("PRIVATE"));
}

#[test]
fn chains_are_described_without_their_key() {
    let details = describe(EC_CHAIN).unwrap();
    assert_eq!(
        details,
        accept(EC_CHAIN, EC_KEY, Utc::now()).unwrap().details
    );
    assert!(describe(WEAK_CHAIN)
        .unwrap_err()
        .message
        .contains("at least 2048 bits"));
}

#[test]
fn chains_presented_in_handshakes_are_described() {
    let leaf = CertificateDer::from_pem_slice(EC_CHAIN.as_bytes()).unwrap();
    assert_eq!(
        describe_der(&[leaf.to_vec()]).unwrap(),
        describe(EC_CHAIN).unwrap()
    );
    assert!(describe_der(&[]).is_err());
}

#[test]
fn sec1_ec_keys_with_parameters_are_accepted() {
    let accepted = accept(EC_CHAIN, EC_KEY, Utc::now()).unwrap();

    assert_eq!(accepted.details.key_algorithm, KeyAlgorithm::EcdsaP256);
    assert_eq!(accepted.details.names, ["ec.example", "192.0.2.1"]);
    assert!(accepted.key.starts_with("-----BEGIN EC PRIVATE KEY-----\n"));
    assert!(!accepted.key.contains("EC PARAMETERS"));
    assert!(accepted.details.covers(&host("ec.example")));
}

#[test]
fn p384_and_ed25519_keys_are_accepted() {
    let not_after = Utc::now() + Duration::days(30);
    for (algorithm, expected, bits) in [
        (&PKCS_ECDSA_P384_SHA384, KeyAlgorithm::EcdsaP384, 384),
        (&PKCS_ED25519, KeyAlgorithm::Ed25519, 256),
    ] {
        let (chain, key) = leaf(&["example.com"], algorithm, None, not_after);
        let details = accept(&chain, &key, Utc::now()).unwrap().details;
        assert_eq!((details.key_algorithm, details.key_bits), (expected, bits));
    }
}

#[test]
fn weak_and_unrelated_keys_are_refused() {
    assert!(refusal(WEAK_CHAIN, RSA_KEY).contains("at least 2048 bits"));
    assert!(refusal(RSA_CHAIN, EC_KEY).contains("does not belong"));
    let not_after = Utc::now() + Duration::days(30);
    let (chain, _) = leaf(&["example.com"], &PKCS_ECDSA_P256_SHA256, None, not_after);
    let (_, other) = leaf(&["example.com"], &PKCS_ECDSA_P256_SHA256, None, not_after);
    assert!(refusal(&chain, &other).contains("does not belong"));
}

#[test]
fn chains_list_the_leaf_first() {
    let root = authority("Root", None);
    let intermediate = authority("Intermediate", Some(&root));
    let not_after = Utc::now() + Duration::days(60);
    let (certificate, key) = leaf(
        &["www.example.com"],
        &PKCS_ECDSA_P256_SHA256,
        Some(&intermediate),
        not_after,
    );

    let ordered = format!("{certificate}{}{}", intermediate.pem, root.pem);
    let details = accept(&ordered, &key, Utc::now()).unwrap().details;
    assert_eq!(details.chain_length, 3);
    assert_eq!(details.issuer, "CN=Intermediate");
    assert!(!details.self_signed);
    assert_eq!(details.status(Utc::now()), CertificateStatus::Valid);

    let reversed = format!("{certificate}{}{}", root.pem, intermediate.pem);
    assert!(refusal(&reversed, &key).contains("did not issue certificate 1"));
}

#[test]
fn expired_and_nameless_certificates_are_refused() {
    let (expired, key) = leaf(
        &["example.com"],
        &PKCS_ECDSA_P256_SHA256,
        None,
        Utc::now() - Duration::days(1),
    );
    assert!(refusal(&expired, &key).contains("expired on"));

    let (nameless, key) = leaf(
        &[],
        &PKCS_ECDSA_P256_SHA256,
        None,
        Utc::now() + Duration::days(9),
    );
    assert!(refusal(&nameless, &key).contains("names no DNS name"));
}

#[test]
fn malformed_input_is_refused_with_its_reason() {
    let encrypted = pem("ENCRYPTED PRIVATE KEY", b"opaque");
    for (chain, key, reason) in [
        ("", RSA_KEY, "holds no PEM certificate"),
        ("not pem at all", RSA_KEY, "holds no PEM certificate"),
        (
            &format!("{RSA_CHAIN}{RSA_KEY}"),
            RSA_KEY,
            "holds a RSA PRIVATE KEY block",
        ),
        (RSA_CHAIN, "", "exactly one PEM key"),
        (
            RSA_CHAIN,
            &format!("{RSA_KEY}{RSA_KEY}"),
            "exactly one PEM key",
        ),
        (RSA_CHAIN, &encrypted, "encrypted; decrypt it first"),
        (
            RSA_CHAIN,
            RSA_CHAIN,
            "CERTIFICATE block is not a private key",
        ),
        (
            &pem("CERTIFICATE", b"garbage"),
            RSA_KEY,
            "not a DER X.509 certificate",
        ),
    ] {
        let message = refusal(chain, key);
        assert!(message.contains(reason), "{reason}: {message}");
    }
}

#[test]
fn self_signed_certificates_cover_their_names() {
    let now = Utc::now();
    let names = [
        "Example.com".to_owned(),
        "*.example.com".into(),
        "10.0.0.1".into(),
    ];
    let generated = self_signed(&names, 90, now).unwrap();
    let details = &generated.details;

    assert_eq!(details.names, ["example.com", "*.example.com", "10.0.0.1"]);
    assert_eq!(details.subject, "CN=example.com");
    assert!(details.self_signed);
    assert_eq!(details.key_algorithm, KeyAlgorithm::EcdsaP256);
    assert_eq!(
        details.not_after.timestamp(),
        (now + Duration::days(90)).timestamp()
    );
    assert!(details.not_before < now);
    assert!(details.covers(&host("www.example.com")));
    assert!(generated.key.starts_with("-----BEGIN PRIVATE KEY-----\n"));
    assert_eq!(
        accept(&generated.chain, &generated.key, now)
            .unwrap()
            .details,
        generated.details
    );
    let again = self_signed(&names, 90, now).unwrap().details;
    assert_ne!(again.serial, details.serial);

    for (names, days, reason) in [
        (vec![], 90, "1 to 100 names"),
        (vec!["example.com".to_owned()], 0, "1 to 825 days"),
        (
            vec!["example.com".to_owned()],
            MAX_SELF_SIGNED_DAYS + 1,
            "1 to 825 days",
        ),
        (vec!["exa mple.com".to_owned()], 90, "is not a host name"),
    ] {
        let message = self_signed(&names, days, now).unwrap_err().message;
        assert!(message.contains(reason), "{reason}: {message}");
    }
}
