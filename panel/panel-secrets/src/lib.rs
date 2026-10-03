#![forbid(unsafe_code)]

//! Sealing of secret material, such as private keys, for storage.
//!
//! [`EnvelopeVault`] encrypts every value with its own random data key under
//! AES-256-GCM and encrypts that key with a master key from the deployment's
//! secrets. The owner of a value is bound in as associated data, so a sealed
//! value opens only for the record it was sealed for. Other providers, such
//! as a key management service, implement [`SecretVault`].

use async_trait::async_trait;
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use panel_errors::{PanelError, Result};
use ring::{
    aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN},
    rand::{SecureRandom, SystemRandom},
};
use sha2::{Digest, Sha256};
use std::fmt;
use zeroize::Zeroizing;

const FORMAT: &str = "v1";
const CONTEXT: &[u8] = b"pingora-panel sealed secret v1\0";
const KEY_ID_CONTEXT: &[u8] = b"pingora-panel master key\0";
const KEY_LEN: usize = 32;

/// A sealed value, safe to store: it names the master key that sealed it and
/// opens only for its owner.
#[derive(Clone, Eq, PartialEq)]
pub struct Sealed(String);

impl Sealed {
    /// Takes a stored value; [`SecretVault::open`] checks it.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The fingerprint of the master key that sealed the value.
    pub fn key_id(&self) -> Option<&str> {
        self.0.split('.').nth(1)
    }
}

impl fmt::Debug for Sealed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("Sealed")
            .field(&self.key_id().unwrap_or("?"))
            .finish()
    }
}

/// Seals and opens secret material.
#[async_trait]
pub trait SecretVault: Send + Sync {
    /// Seals `plaintext` for `owner`, a stable name such as
    /// `certificate/<id>/key`.
    async fn seal(&self, owner: &str, plaintext: &[u8]) -> Result<Sealed>;

    /// Opens a value sealed for `owner`.
    async fn open(&self, owner: &str, sealed: &Sealed) -> Result<Zeroizing<Vec<u8>>>;

    /// Whether `sealed` uses the key new values are sealed with; values that
    /// do not should be sealed again before their key is retired.
    fn is_current(&self, sealed: &Sealed) -> bool;
}

struct MasterKey {
    id: String,
    key: LessSafeKey,
}

/// Envelope encryption under master keys held by the process.
pub struct EnvelopeVault {
    /// The first seals; all of them open.
    keys: Vec<MasterKey>,
    random: SystemRandom,
}

impl fmt::Debug for EnvelopeVault {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EnvelopeVault")
            .field(
                "keys",
                &self.keys.iter().map(|key| &key.id).collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl EnvelopeVault {
    /// Reads master keys, one base64-encoded 256-bit key per line; the first
    /// seals new values. Blank lines and lines starting with `#` are skipped.
    pub fn from_keys(text: &str) -> Result<Self> {
        let mut keys: Vec<MasterKey> = Vec::new();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let material = Zeroizing::new(STANDARD.decode(line).map_err(|_| {
                PanelError::invalid_argument(format!(
                    "master key on line {} is not base64",
                    index + 1
                ))
            })?);
            if material.len() != KEY_LEN {
                return Err(PanelError::invalid_argument(format!(
                    "master key on line {} has {} bytes instead of {KEY_LEN}",
                    index + 1,
                    material.len()
                )));
            }
            let id = key_id(&material);
            if keys.iter().any(|key| key.id == id) {
                return Err(PanelError::invalid_argument(format!(
                    "master key on line {} is listed twice",
                    index + 1
                )));
            }
            keys.push(MasterKey {
                id,
                key: aead_key(&material)?,
            });
        }
        if keys.is_empty() {
            return Err(PanelError::invalid_argument("no master key is configured"));
        }
        Ok(Self {
            keys,
            random: SystemRandom::new(),
        })
    }

    /// A new master key as a line for [`EnvelopeVault::from_keys`].
    pub fn generate_key() -> Result<String> {
        let mut material = Zeroizing::new([0_u8; KEY_LEN]);
        SystemRandom::new()
            .fill(material.as_mut())
            .map_err(|_| PanelError::internal("no randomness for a master key"))?;
        Ok(STANDARD.encode(material.as_ref()))
    }

    /// The fingerprint of the key that seals new values.
    pub fn active_key_id(&self) -> &str {
        &self.keys[0].id
    }

    fn nonce(&self) -> Result<[u8; NONCE_LEN]> {
        let mut nonce = [0_u8; NONCE_LEN];
        self.random
            .fill(&mut nonce)
            .map_err(|_| PanelError::internal("no randomness for sealing"))?;
        Ok(nonce)
    }

    fn encrypt(&self, key: &LessSafeKey, owner: &str, data: &[u8]) -> Result<String> {
        let nonce = self.nonce()?;
        let mut buffer = Zeroizing::new(Vec::with_capacity(NONCE_LEN + data.len() + 16));
        buffer.extend_from_slice(data);
        key.seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(associated_data(owner)),
            &mut *buffer,
        )
        .map_err(|_| PanelError::internal("secret material could not be sealed"))?;
        buffer.splice(0..0, nonce);
        Ok(URL_SAFE_NO_PAD.encode(buffer.as_slice()))
    }
}

#[async_trait]
impl SecretVault for EnvelopeVault {
    async fn seal(&self, owner: &str, plaintext: &[u8]) -> Result<Sealed> {
        let mut data_key = Zeroizing::new([0_u8; KEY_LEN]);
        self.random
            .fill(data_key.as_mut())
            .map_err(|_| PanelError::internal("no randomness for a data key"))?;
        let master = &self.keys[0];
        let wrapped = self.encrypt(&master.key, owner, data_key.as_ref())?;
        let body = self.encrypt(&aead_key(data_key.as_ref())?, owner, plaintext)?;
        Ok(Sealed(format!("{FORMAT}.{}.{wrapped}.{body}", master.id)))
    }

    async fn open(&self, owner: &str, sealed: &Sealed) -> Result<Zeroizing<Vec<u8>>> {
        let mut parts = sealed.0.split('.');
        let (Some(FORMAT), Some(id), Some(wrapped), Some(body), None) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            return Err(PanelError::corrupt_state("a sealed secret is malformed"));
        };
        let master = self.keys.iter().find(|key| key.id == id).ok_or_else(|| {
            PanelError::precondition_failed(format!(
                "a secret is sealed with master key {id}, which is not configured"
            ))
        })?;
        let data_key = decrypt(&master.key, owner, wrapped)?;
        if data_key.len() != KEY_LEN {
            return Err(PanelError::corrupt_state("a sealed secret is malformed"));
        }
        decrypt(&aead_key(&data_key)?, owner, body)
    }

    fn is_current(&self, sealed: &Sealed) -> bool {
        sealed.key_id() == Some(self.active_key_id())
    }
}

fn decrypt(key: &LessSafeKey, owner: &str, encoded: &str) -> Result<Zeroizing<Vec<u8>>> {
    let malformed = || PanelError::corrupt_state("a sealed secret could not be opened");
    let mut buffer = Zeroizing::new(URL_SAFE_NO_PAD.decode(encoded).map_err(|_| malformed())?);
    if buffer.len() < NONCE_LEN {
        return Err(malformed());
    }
    let nonce: [u8; NONCE_LEN] = buffer[..NONCE_LEN].try_into().map_err(|_| malformed())?;
    let length = key
        .open_in_place(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(associated_data(owner)),
            &mut buffer[NONCE_LEN..],
        )
        .map_err(|_| malformed())?
        .len();
    Ok(Zeroizing::new(
        buffer[NONCE_LEN..NONCE_LEN + length].to_vec(),
    ))
}

fn aead_key(material: &[u8]) -> Result<LessSafeKey> {
    UnboundKey::new(&AES_256_GCM, material)
        .map(LessSafeKey::new)
        .map_err(|_| PanelError::internal("an encryption key has the wrong length"))
}

fn associated_data(owner: &str) -> Vec<u8> {
    [CONTEXT, owner.as_bytes()].concat()
}

fn key_id(material: &[u8]) -> String {
    let digest = Sha256::new()
        .chain_update(KEY_ID_CONTEXT)
        .chain_update(material)
        .finalize();
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_errors::ErrorCode;

    fn vault(keys: &[&str]) -> EnvelopeVault {
        EnvelopeVault::from_keys(&keys.join("\n")).unwrap()
    }

    #[tokio::test]
    async fn sealed_values_open_only_for_their_owner() {
        let key = EnvelopeVault::generate_key().unwrap();
        let vault = vault(&[&key]);
        let sealed = vault.seal("certificate/a/key", b"private").await.unwrap();

        assert_eq!(
            vault
                .open("certificate/a/key", &sealed)
                .await
                .unwrap()
                .as_slice(),
            b"private"
        );
        let moved = vault.open("certificate/b/key", &sealed).await.unwrap_err();
        assert_eq!(moved.code.as_str(), ErrorCode::CORRUPT_STATE);
        assert!(!sealed.as_str().contains("private"));
        assert_ne!(
            vault.seal("certificate/a/key", b"private").await.unwrap(),
            sealed
        );
    }

    #[tokio::test]
    async fn tampered_values_do_not_open() {
        let vault = vault(&[&EnvelopeVault::generate_key().unwrap()]);
        let sealed = vault.seal("owner", b"secret").await.unwrap();
        let mut text = sealed.as_str().to_owned();
        let last = text.pop().unwrap();
        text.push(if last == 'A' { 'B' } else { 'A' });

        for broken in [
            Sealed::new(text),
            Sealed::new("v1.only.three"),
            Sealed::new(sealed.as_str().replacen("v1", "v9", 1)),
        ] {
            let error = vault.open("owner", &broken).await.unwrap_err();
            assert_eq!(error.code.as_str(), ErrorCode::CORRUPT_STATE);
        }
    }

    #[tokio::test]
    async fn master_keys_rotate_without_losing_values() {
        let old = EnvelopeVault::generate_key().unwrap();
        let new = EnvelopeVault::generate_key().unwrap();
        let before = vault(&[&old]);
        let sealed = before.seal("owner", b"secret").await.unwrap();

        let rotating = vault(&[&new, &old]);
        assert!(!rotating.is_current(&sealed));
        assert_eq!(
            rotating.open("owner", &sealed).await.unwrap().as_slice(),
            b"secret"
        );
        let resealed = rotating.seal("owner", b"secret").await.unwrap();
        assert!(rotating.is_current(&resealed));
        assert_eq!(resealed.key_id(), Some(rotating.active_key_id()));

        let retired = vault(&[&new]).open("owner", &sealed).await.unwrap_err();
        assert_eq!(retired.code.as_str(), ErrorCode::PRECONDITION_FAILED);
        assert!(retired.message.contains(before.active_key_id()));
    }

    #[test]
    fn master_keys_are_checked() {
        let key = EnvelopeVault::generate_key().unwrap();
        let vault = EnvelopeVault::from_keys(&format!("# current\n\n{key}\n")).unwrap();
        assert_eq!(vault.active_key_id().len(), 16);
        assert!(!format!("{vault:?}").contains(&key));

        for (text, problem) in [
            ("", "no master key"),
            ("# only a comment", "no master key"),
            ("not base64!", "not base64"),
            ("c2hvcnQ=", "has 5 bytes"),
            (&format!("{key}\n{key}"), "listed twice"),
        ] {
            let error = EnvelopeVault::from_keys(text).unwrap_err();
            assert!(error.message.contains(problem), "{text}: {}", error.message);
        }
    }
}
