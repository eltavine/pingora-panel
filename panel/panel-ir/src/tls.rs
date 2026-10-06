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

/// The OpenSSL names of the TLS 1.2 suites, by their IANA names.
pub const OPENSSL_SUITES: &[(&str, &str)] = &[
    (
        "ECDHE-ECDSA-AES256-GCM-SHA384",
        "TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384",
    ),
    (
        "ECDHE-ECDSA-AES128-GCM-SHA256",
        "TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256",
    ),
    (
        "ECDHE-ECDSA-CHACHA20-POLY1305",
        "TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256",
    ),
    (
        "ECDHE-RSA-AES256-GCM-SHA384",
        "TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384",
    ),
    (
        "ECDHE-RSA-AES128-GCM-SHA256",
        "TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256",
    ),
    (
        "ECDHE-RSA-CHACHA20-POLY1305",
        "TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256",
    ),
];

/// The TLS 1.2 suites an OpenSSL cipher list chooses, by IANA name, or
/// none to keep them all. The list chooses among the suites of
/// [`OPENSSL_SUITES`] by their names; other names and keywords such as
/// `HIGH` keep them all, `!name` and `-name` leave one out and `+name` and
/// `@` options change nothing. TLS 1.3 suites are not in it, as OpenSSL
/// keeps them apart.
pub fn openssl_suites(list: &str) -> Result<Vec<String>, String> {
    let mut named = Vec::new();
    let mut excluded = Vec::new();
    for token in list
        .split([':', ',', ' '])
        .filter(|token| !token.is_empty())
    {
        let (exclude, name) = match token.as_bytes()[0] {
            b'!' | b'-' => (true, &token[1..]),
            b'+' | b'@' => continue,
            _ => (false, token),
        };
        if let Some((_, iana)) = OPENSSL_SUITES.iter().find(|(openssl, _)| *openssl == name) {
            if exclude {
                excluded.push(*iana);
            } else if !named.contains(iana) {
                named.push(*iana);
            }
        }
    }
    let chosen: Vec<&str> = if named.is_empty() {
        OPENSSL_SUITES.iter().map(|(_, iana)| *iana).collect()
    } else {
        named
    };
    let left: Vec<String> = chosen
        .into_iter()
        .filter(|suite| !excluded.contains(suite))
        .map(str::to_owned)
        .collect();
    if left.is_empty() {
        return Err(format!(
            "{list:?} leaves out every TLS 1.2 suite: {}",
            OPENSSL_SUITES
                .iter()
                .map(|(openssl, _)| *openssl)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok(if left.len() == OPENSSL_SUITES.len() {
        Vec::new()
    } else {
        left
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openssl_lists_choose_among_the_suites_offered() {
        assert!(openssl_suites("HIGH:!aNULL:!MD5").unwrap().is_empty());
        assert!(openssl_suites("DEFAULT").unwrap().is_empty());
        assert_eq!(
            openssl_suites("ECDHE-RSA-AES128-GCM-SHA256:AES128-SHA:+RSA").unwrap(),
            ["TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256"]
        );
        assert_eq!(
            openssl_suites("ALL:!ECDHE-ECDSA-AES256-GCM-SHA384")
                .unwrap()
                .len(),
            OPENSSL_SUITES.len() - 1
        );
        let all: Vec<_> = OPENSSL_SUITES
            .iter()
            .map(|(openssl, _)| format!("!{openssl}"))
            .collect();
        assert!(openssl_suites(&all.join(":")).is_err());
        assert!(OPENSSL_SUITES
            .iter()
            .all(|(_, iana)| suite_version(iana) == Some(SuiteVersion::Tls12)));
    }
}
