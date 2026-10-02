//! SCRAM-SHA-256 password verifiers (RFC 5802, RFC 7677).
//!
//! Passwords are hashed by the client and sent to PostgreSQL as verifiers,
//! so the server never receives or logs the plaintext.

use base64::{engine::general_purpose::STANDARD, Engine};
use hmac::{Hmac, KeyInit, Mac};
use panel_errors::{PanelError, Result};
use sha2::{Digest, Sha256};
use std::fmt;

/// PostgreSQL's default `scram_iterations`.
pub const DEFAULT_ITERATIONS: u32 = 4096;
const SALT_BYTES: usize = 16;
const GENERATED_SECRET_BYTES: usize = 32;

/// A role password. The value is never printed by `Debug`.
#[derive(Clone, Eq, PartialEq)]
pub struct RoleSecret(String);

impl RoleSecret {
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.is_empty() || value.len() > 1024 {
            return Err(PanelError::invalid_argument(
                "role secrets must contain 1..=1024 bytes",
            ));
        }
        Ok(Self(value))
    }

    /// A random secret with 256 bits of entropy.
    pub fn generate() -> Result<Self> {
        let mut bytes = [0_u8; GENERATED_SECRET_BYTES];
        getrandom::fill(&mut bytes)
            .map_err(|error| PanelError::internal(format!("entropy unavailable: {error}")))?;
        Ok(Self(
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes),
        ))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for RoleSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("RoleSecret(<redacted>)")
    }
}

/// A stored SCRAM-SHA-256 verifier in PostgreSQL's
/// `SCRAM-SHA-256$<iterations>:<salt>$<StoredKey>:<ServerKey>` form.
#[derive(Clone, Eq, PartialEq)]
pub struct ScramVerifier(String);

impl ScramVerifier {
    pub fn derive(secret: &RoleSecret) -> Result<Self> {
        let mut salt = [0_u8; SALT_BYTES];
        getrandom::fill(&mut salt)
            .map_err(|error| PanelError::internal(format!("entropy unavailable: {error}")))?;
        Ok(Self::derive_with(secret, &salt, DEFAULT_ITERATIONS))
    }

    /// Deterministic derivation for a given salt and iteration count.
    pub fn derive_with(secret: &RoleSecret, salt: &[u8], iterations: u32) -> Self {
        // SASLprep normalizes the password as PostgreSQL does; a password it
        // cannot prepare is used as raw bytes, matching the server.
        let normalized = stringprep::saslprep(secret.expose())
            .map(|value| value.into_owned())
            .unwrap_or_else(|_| secret.expose().to_owned());
        let mut salted = [0_u8; 32];
        pbkdf2::pbkdf2_hmac::<Sha256>(normalized.as_bytes(), salt, iterations, &mut salted);
        let client_key = hmac(&salted, b"Client Key");
        let stored_key = Sha256::digest(client_key);
        let server_key = hmac(&salted, b"Server Key");
        Self(format!(
            "SCRAM-SHA-256${iterations}:{}${}:{}",
            STANDARD.encode(salt),
            STANDARD.encode(stored_key),
            STANDARD.encode(server_key)
        ))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ScramVerifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ScramVerifier(<redacted>)")
    }
}

fn hmac(key: &[u8], message: &[u8]) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts keys of any length");
    mac.update(message);
    mac.finalize().into_bytes().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifier_reproduces_the_rfc_7677_exchange() {
        // RFC 7677 section 3 authenticates "user" with password "pencil".
        // The derived keys must reproduce both proofs of that exchange.
        let salt = STANDARD.decode("W22ZaJ0SNY7soEsUEjb6gQ==").unwrap();
        let verifier = ScramVerifier::derive_with(&RoleSecret::new("pencil").unwrap(), &salt, 4096);
        assert!(verifier
            .as_str()
            .starts_with("SCRAM-SHA-256$4096:W22ZaJ0SNY7soEsUEjb6gQ==$"));
        let (_, keys) = verifier.as_str().rsplit_once('$').unwrap();
        let (stored_key, server_key) = keys.split_once(':').unwrap();
        let stored_key = STANDARD.decode(stored_key).unwrap();
        let server_key = STANDARD.decode(server_key).unwrap();
        let nonce = "rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0";
        let auth_message = format!(
            "n=user,r=rOprNGfwEbeRWgbNEkqO,r={nonce},s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096,c=biws,r={nonce}"
        );

        assert_eq!(
            STANDARD.encode(hmac(&server_key, auth_message.as_bytes())),
            "6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4="
        );
        let proof = STANDARD
            .decode("dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ=")
            .unwrap();
        let client_key = proof
            .iter()
            .zip(hmac(&stored_key, auth_message.as_bytes()))
            .map(|(proof, signature)| proof ^ signature)
            .collect::<Vec<_>>();
        assert_eq!(
            Sha256::digest(&client_key).as_slice(),
            stored_key.as_slice()
        );
    }

    #[test]
    fn secrets_are_random_and_redacted() {
        let first = RoleSecret::generate().unwrap();
        let second = RoleSecret::generate().unwrap();
        assert_ne!(first, second);
        assert_eq!(first.expose().len(), 43);
        assert_eq!(format!("{first:?}"), "RoleSecret(<redacted>)");
        assert_eq!(
            format!("{:?}", ScramVerifier::derive(&first).unwrap()),
            "ScramVerifier(<redacted>)"
        );
        assert!(RoleSecret::new("").is_err());
    }
}
