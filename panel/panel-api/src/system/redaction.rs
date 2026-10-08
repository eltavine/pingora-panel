//! What a diagnostic bundle must not carry: values under the names of
//! secrets, and PEM blocks, credentials in `Authorization` schemes, the
//! panel's own tokens, JSON Web Tokens and passwords in URLs wherever they
//! appear.

use regex::Regex;
use serde_json::Value;
use std::{borrow::Cow, sync::LazyLock};

/// What replaces a removed value.
pub(crate) const REDACTED: &str = "[redacted]";

/// Words that make a name one of a secret, compared with the words of
/// `snake_case`, `kebab-case`, dotted and `camelCase` names.
const SECRET_WORDS: &[&str] = &[
    "apikey",
    "authorization",
    "bearer",
    "cookie",
    "cookies",
    "credential",
    "credentials",
    "key",
    "keys",
    "otp",
    "passphrase",
    "passwd",
    "password",
    "passwords",
    "pepper",
    "private",
    "secret",
    "secrets",
    "session",
    "sessions",
    "signature",
    "token",
    "tokens",
    "totp",
];

/// Token-shaped text and what replaces it, in the order they apply.
static PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        // A truncated block has no end line; everything after its start goes.
        (
            r"(?s)-----BEGIN [A-Z0-9 ]+-----(?:.*?-----END [A-Z0-9 ]+-----|.*)",
            "[redacted PEM block]",
        ),
        (r"(?i)\b(bearer)\s+[A-Za-z0-9._~+/=-]+", "$1 [redacted]"),
        (
            r"(?i)\b(authorization:\s*[a-z0-9-]+\s+)[^\s,;]+",
            "${1}[redacted]",
        ),
        (r"\b(?:ppat|whsec)_[A-Za-z0-9_-]+", REDACTED),
        (
            r"\beyJ[A-Za-z0-9_-]*\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]*",
            REDACTED,
        ),
        (
            r"(?i)\b([a-z][a-z0-9+.-]*://[^/\s:@]+):[^/\s@]+@",
            "$1:[redacted]@",
        ),
        (
            r"(?i)([?&][a-z0-9_.-]*(?:token|secret|password|passwd|key|signature|sig|auth|credential)[a-z0-9_.-]*)=[^&\s#]+",
            "$1=[redacted]",
        ),
    ]
    .into_iter()
    .map(|(pattern, replacement)| {
        (
            Regex::new(pattern).expect("the redaction patterns compile"),
            replacement,
        )
    })
    .collect()
});

/// Removes every secret from `value` in place.
pub(crate) fn redact(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (name, value) in fields.iter_mut() {
                if names_a_secret(name) && !matches!(value, Value::Null | Value::Bool(_)) {
                    *value = Value::String(REDACTED.into());
                } else {
                    redact(value);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact),
        Value::String(text) => mask(text),
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn mask(text: &mut String) {
    for (pattern, replacement) in PATTERNS.iter() {
        if let Cow::Owned(masked) = pattern.replace_all(text, *replacement) {
            *text = masked;
        }
    }
}

fn names_a_secret(name: &str) -> bool {
    words(name)
        .iter()
        .any(|word| SECRET_WORDS.contains(&word.as_str()))
}

/// The lowercase words of a name, split at anything but letters and digits
/// and where a lowercase letter or digit meets an uppercase one.
fn words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut after_lower = false;
    for character in name.chars() {
        if !character.is_alphanumeric() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            after_lower = false;
            continue;
        }
        if character.is_uppercase() && after_lower && !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
        after_lower = character.is_lowercase() || character.is_ascii_digit();
        word.extend(character.to_lowercase());
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn redacted(mut value: Value) -> Value {
        redact(&mut value);
        value
    }

    #[test]
    fn names_split_into_words_of_every_convention() {
        assert_eq!(words("client_secret"), ["client", "secret"]);
        assert_eq!(words("privateKeyPem"), ["private", "key", "pem"]);
        assert_eq!(words("X-Api-Key"), ["x", "api", "key"]);
        assert_eq!(words("APIKey"), ["apikey"]);
        assert_eq!(words("tls.key2Path"), ["tls", "key2", "path"]);
    }

    #[test]
    fn values_under_the_names_of_secrets_are_replaced() {
        let bundle = redacted(json!({
            "password": "hunter2",
            "client_secret": {"nested": "value"},
            "apiKey": 42,
            "Set-Cookie": ["a=b"],
            "sessionId": "s-1",
            "has_password": true,
            "token": null,
            "monkey": "kept",
            "keystone": "kept",
            "previous_hash": "abc",
            "items": [{"authorization": "Basic Zm9v"}, {"name": "kept"}],
        }));
        assert_eq!(
            bundle,
            json!({
                "password": REDACTED,
                "client_secret": REDACTED,
                "apiKey": REDACTED,
                "Set-Cookie": REDACTED,
                "sessionId": REDACTED,
                "has_password": true,
                "token": null,
                "monkey": "kept",
                "keystone": "kept",
                "previous_hash": "abc",
                "items": [{"authorization": REDACTED}, {"name": "kept"}],
            })
        );
    }

    #[test]
    fn token_shaped_text_is_masked_wherever_it_appears() {
        let pem = "-----BEGIN PRIVATE KEY-----\nMIIEvQ\n-----END PRIVATE KEY-----";
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2lnbmF0dXJl";
        let bundle = redacted(json!({
            "message": format!("loaded {pem} for site"),
            "truncated": "key -----BEGIN CERTIFICATE-----\nMIIB",
            "header": "Authorization: Bearer abc.def-123",
            "request": "GET / with Authorization: Basic Zm9vOmJhcg==, refused",
            "detail": "issued ppat_0123abcd and whsec_c2VjcmV0 to the hook",
            "error": format!("rejected {jwt}"),
            "url": "https://admin:hunter2@hooks.example/notify?token=abc&site=1&api_key=xyz",
        }));
        assert_eq!(
            bundle,
            json!({
                "message": "loaded [redacted PEM block] for site",
                "truncated": "key [redacted PEM block]",
                "header": "Authorization: Bearer [redacted]",
                "request": "GET / with Authorization: Basic [redacted], refused",
                "detail": "issued [redacted] and [redacted] to the hook",
                "error": "rejected [redacted]",
                "url": "https://admin:[redacted]@hooks.example/notify?token=[redacted]&site=1&api_key=[redacted]",
            })
        );
    }

    #[test]
    fn text_without_secrets_is_left_as_it_is() {
        let text = json!({
            "message": "upstream 10.0.0.1:8080 refused the connection",
            "check": "basic checks passed",
            "url": "https://hooks.example/notify?site=1",
            "version": "0.9.0",
        });
        assert_eq!(redacted(text.clone()), text);
    }
}
