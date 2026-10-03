//! Random secrets and the hashes stored in their place.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use panel_errors::{PanelError, Result};
use sha2::{Digest, Sha256};
use std::fmt;
use subtle::ConstantTimeEq;

/// API tokens start with this, so they are recognizable wherever they leak.
pub const TOKEN_PREFIX: &str = "ppat_";

/// 256 random bits as unpadded base64url text. Only its hash is stored.
#[derive(Clone, Eq, PartialEq)]
pub struct Secret(String);

impl Secret {
    pub fn generate() -> Result<Self> {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes)
            .map_err(|error| PanelError::internal(format!("no random bytes: {error}")))?;
        Ok(Self(URL_SAFE_NO_PAD.encode(bytes)))
    }

    /// A new API token: the prefix and a fresh secret.
    pub fn token() -> Result<Self> {
        Ok(Self(format!("{TOKEN_PREFIX}{}", Self::generate()?.0)))
    }

    /// The text to hand to its owner, once.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn hash(&self) -> SecretHash {
        SecretHash::of(&self.0)
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Secret(..)")
    }
}

/// The SHA-256 of a secret's text. Secrets carry 256 random bits, so a fast
/// hash is enough to make the stored value useless to whoever reads it.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct SecretHash([u8; 32]);

impl SecretHash {
    pub fn of(text: &str) -> Self {
        Self(Sha256::digest(text.as_bytes()).into())
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        bytes
            .try_into()
            .map(Self)
            .map_err(|_| PanelError::corrupt_state("a stored secret hash is not 32 bytes"))
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Compares in constant time.
    pub fn matches(&self, other: &Self) -> bool {
        self.0.ct_eq(&other.0).into()
    }
}

impl fmt::Debug for SecretHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretHash(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_random_and_only_their_hash_matches() {
        let first = Secret::generate().unwrap();
        let second = Secret::generate().unwrap();
        assert_ne!(first, second);
        assert_eq!(first.expose().len(), 43);
        assert!(first.hash().matches(&SecretHash::of(first.expose())));
        assert!(!first.hash().matches(&second.hash()));
        assert_eq!(format!("{first:?}"), "Secret(..)");

        let token = Secret::token().unwrap();
        assert!(token.expose().starts_with(TOKEN_PREFIX));
        assert_eq!(
            SecretHash::from_bytes(token.hash().as_bytes()).unwrap(),
            token.hash()
        );
        assert!(SecretHash::from_bytes(&[0; 4]).is_err());
    }
}
