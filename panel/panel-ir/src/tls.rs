//! TLS names snapshots use, independent of any TLS library.

/// Required by snapshots whose TLS profiles cap the protocol version, name
/// cipher suites or turn session resumption off.
pub const TLS_SETTINGS_CAPABILITY: &str = "listener.tls-settings";

/// Required by snapshots that send Strict-Transport-Security.
pub const HSTS_CAPABILITY: &str = "response.hsts";

/// Protocol versions a profile may name, oldest first.
pub const PROTOCOLS: &[&str] = &["TLSv1.2", "TLSv1.3"];

/// The protocol version a cipher suite belongs to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SuiteVersion {
    Tls12,
    Tls13,
}

/// Cipher suites a profile may name, by IANA name: the AEAD suites with
/// forward secrecy that TLS 1.3 defines and TLS 1.2 offers with ECDHE.
pub const CIPHER_SUITES: &[(&str, SuiteVersion)] = &[
    ("TLS13_AES_256_GCM_SHA384", SuiteVersion::Tls13),
    ("TLS13_AES_128_GCM_SHA256", SuiteVersion::Tls13),
    ("TLS13_CHACHA20_POLY1305_SHA256", SuiteVersion::Tls13),
    (
        "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384",
        SuiteVersion::Tls12,
    ),
    (
        "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256",
        SuiteVersion::Tls12,
    ),
    (
        "TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256",
        SuiteVersion::Tls12,
    ),
    ("TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384", SuiteVersion::Tls12),
    ("TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256", SuiteVersion::Tls12),
    (
        "TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256",
        SuiteVersion::Tls12,
    ),
];

/// The version a named cipher suite belongs to, if the name is known.
pub fn suite_version(name: &str) -> Option<SuiteVersion> {
    CIPHER_SUITES
        .iter()
        .find(|(known, _)| *known == name)
        .map(|(_, version)| *version)
}
