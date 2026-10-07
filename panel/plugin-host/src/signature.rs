//! Minisign signatures over `plugin.json` by publishers an administrator
//! trusts (ADR 0044).

use minisign_verify::{PublicKey, Signature};
use serde::{Deserialize, Serialize};

/// A publisher's minisign public key the host trusts.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TrustedKey {
    /// The administrator's name for it.
    pub id: String,
    /// The key's Base64 line, as the second line of `minisign.pub`.
    pub public_key: String,
    #[serde(default)]
    pub comment: String,
}

/// A public key read from its Base64 line or from a whole `minisign.pub`.
pub fn public_key(text: &str) -> Result<(String, String), String> {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("untrusted comment:"))
        .unwrap_or_default();
    PublicKey::from_base64(line)
        .map_err(|error| format!("{line:?} is not a minisign public key: {error}"))?;
    Ok((line.to_owned(), key_id(line)?))
}

/// The key's ID as `minisign` prints it: 16 hexadecimal digits.
pub fn key_id(public_key: &str) -> Result<String, String> {
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(public_key)
        .map_err(|error| format!("not Base64: {error}"))?;
    let id: [u8; 8] = bytes
        .get(2..10)
        .and_then(|id| id.try_into().ok())
        .ok_or("too short for a minisign public key")?;
    Ok(format!("{:016X}", u64::from_le_bytes(id)))
}

/// The trusted key that signed `manifest` with `signature`, or why none did.
pub fn signer<'k>(
    manifest: &[u8],
    signature: &str,
    keys: &'k [TrustedKey],
) -> Result<&'k TrustedKey, String> {
    let signature = Signature::decode(signature)
        .map_err(|error| format!("plugin.json.minisig is not a minisign signature: {error}"))?;
    keys.iter()
        .find(|key| {
            PublicKey::from_base64(&key.public_key)
                .is_ok_and(|public| public.verify(manifest, &signature, false).is_ok())
        })
        .ok_or_else(|| "plugin.json is not signed by a trusted key".to_owned())
}

#[cfg(test)]
pub(crate) mod testing {
    //! Signs as minisign does, for tests: Ed25519 over the BLAKE2b-512 of
    //! the content, then over the signature and its trusted comment.

    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use blake2::{Blake2b512, Digest};
    use ring::signature::{Ed25519KeyPair, KeyPair};

    pub struct SigningKey {
        pair: Ed25519KeyPair,
        id: [u8; 8],
    }

    impl SigningKey {
        pub fn new(seed: u8) -> Self {
            Self {
                pair: Ed25519KeyPair::from_seed_unchecked(&[seed; 32]).unwrap(),
                id: [seed; 8],
            }
        }

        /// The public key's Base64 line.
        pub fn public_key(&self) -> String {
            let mut bytes = b"Ed".to_vec();
            bytes.extend_from_slice(&self.id);
            bytes.extend_from_slice(self.pair.public_key().as_ref());
            STANDARD.encode(bytes)
        }

        /// A `.minisig` file for `content`.
        pub fn sign(&self, content: &[u8]) -> String {
            let hash = Blake2b512::digest(content);
            let signature = self.pair.sign(&hash);
            let mut line = b"ED".to_vec();
            line.extend_from_slice(&self.id);
            line.extend_from_slice(signature.as_ref());
            let comment = "timestamp:1700000000\tfile:plugin.json";
            let mut global = signature.as_ref().to_vec();
            global.extend_from_slice(comment.as_bytes());
            format!(
                "untrusted comment: signature from a test key\n{}\ntrusted comment: {comment}\n{}\n",
                STANDARD.encode(line),
                STANDARD.encode(self.pair.sign(&global))
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{testing::SigningKey, *};

    #[test]
    fn manifests_signed_by_a_trusted_key_verify() {
        let publisher = SigningKey::new(7);
        let stranger = SigningKey::new(9);
        let keys = vec![TrustedKey {
            id: "acme".into(),
            public_key: publisher.public_key(),
            comment: String::new(),
        }];
        let manifest = br#"{"name": "dns"}"#;
        assert_eq!(
            signer(manifest, &publisher.sign(manifest), &keys)
                .unwrap()
                .id,
            "acme"
        );
        assert!(signer(b"{}", &publisher.sign(manifest), &keys).is_err());
        assert_eq!(
            signer(manifest, &stranger.sign(manifest), &keys).unwrap_err(),
            "plugin.json is not signed by a trusted key"
        );
        assert!(signer(manifest, "garbage", &keys)
            .unwrap_err()
            .contains("not a minisign signature"));
    }

    #[test]
    fn public_keys_read_from_lines_or_files() {
        let key = SigningKey::new(7).public_key();
        let (line, id) =
            public_key(&format!("untrusted comment: minisign public key\n{key}\n")).unwrap();
        assert_eq!(line, key);
        assert_eq!(id, "0707070707070707");
        assert!(public_key("RWQnot-a-key").is_err());
    }
}
