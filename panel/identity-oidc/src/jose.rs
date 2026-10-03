//! JSON Web Signatures in the compact serialization (RFC 7515) verified with
//! JSON Web Keys (RFC 7517, RFC 8037) for the asymmetric algorithms of RFC
//! 7518 and EdDSA. Symmetric algorithms and `none` are never accepted: the
//! panel only verifies what a provider signed with a key it published.

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use panel_errors::{PanelError, Result};
use ring::signature::{self, RsaPublicKeyComponents, UnparsedPublicKey, VerificationAlgorithm};
use serde::Deserialize;
use serde_json::Value;

/// A signature algorithm the panel accepts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Algorithm {
    Rs256,
    Rs384,
    Rs512,
    Ps256,
    Ps384,
    Ps512,
    Es256,
    Es384,
    EdDsa,
}

impl Algorithm {
    pub const ALL: [Self; 9] = [
        Self::Rs256,
        Self::Rs384,
        Self::Rs512,
        Self::Ps256,
        Self::Ps384,
        Self::Ps512,
        Self::Es256,
        Self::Es384,
        Self::EdDsa,
    ];

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|algorithm| algorithm.name() == name)
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Rs256 => "RS256",
            Self::Rs384 => "RS384",
            Self::Rs512 => "RS512",
            Self::Ps256 => "PS256",
            Self::Ps384 => "PS384",
            Self::Ps512 => "PS512",
            Self::Es256 => "ES256",
            Self::Es384 => "ES384",
            Self::EdDsa => "EdDSA",
        }
    }

    const fn key_type(self) -> &'static str {
        match self {
            Self::Rs256 | Self::Rs384 | Self::Rs512 | Self::Ps256 | Self::Ps384 | Self::Ps512 => {
                "RSA"
            }
            Self::Es256 | Self::Es384 => "EC",
            Self::EdDsa => "OKP",
        }
    }
}

/// A public key as a provider publishes it.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub struct Jwk {
    pub kty: String,
    #[serde(default)]
    pub kid: Option<String>,
    #[serde(default, rename = "use")]
    pub usage: Option<String>,
    #[serde(default)]
    pub alg: Option<String>,
    #[serde(default)]
    pub n: Option<String>,
    #[serde(default)]
    pub e: Option<String>,
    #[serde(default)]
    pub crv: Option<String>,
    #[serde(default)]
    pub x: Option<String>,
    #[serde(default)]
    pub y: Option<String>,
}

/// A provider's published keys.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
pub struct JwkSet {
    pub keys: Vec<Jwk>,
}

/// A JWS whose signature was verified.
#[derive(Clone, Debug, PartialEq)]
pub struct Verified {
    pub algorithm: Algorithm,
    /// The key that verified it, by `kid` when it has one.
    pub key_id: Option<String>,
    pub payload: Vec<u8>,
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    #[serde(default)]
    kid: Option<String>,
    #[serde(default)]
    crit: Option<Value>,
}

fn invalid(message: impl Into<String>) -> PanelError {
    PanelError::unauthenticated(message)
}

fn decode(part: &str, what: &str) -> Result<Vec<u8>> {
    URL_SAFE_NO_PAD
        .decode(part)
        .map_err(|_| invalid(format!("the token's {what} is not base64url")))
}

/// Verifies a compact JWS against `keys`, accepting only `allowed`
/// algorithms.
pub fn verify(token: &str, keys: &JwkSet, allowed: &[Algorithm]) -> Result<Verified> {
    let mut parts = token.split('.');
    let (Some(header), Some(payload), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(invalid("the token is not a compact JWS"));
    };
    let header_json: Header = serde_json::from_slice(&decode(header, "header")?)
        .map_err(|_| invalid("the token's header is not a JWS header"))?;
    if header_json.crit.is_some() {
        return Err(invalid("the token names critical extensions"));
    }
    let algorithm = Algorithm::parse(&header_json.alg)
        .filter(|algorithm| allowed.contains(algorithm))
        .ok_or_else(|| {
            invalid(format!(
                "the token's algorithm {:?} is not accepted",
                header_json.alg
            ))
        })?;
    let signature = decode(signature, "signature")?;
    let payload_bytes = decode(payload, "payload")?;
    let signed = &token.as_bytes()[..header.len() + 1 + payload.len()];
    let key = select(keys, algorithm, header_json.kid.as_deref())?;
    check(key, algorithm, signed, &signature)?;
    Ok(Verified {
        algorithm,
        key_id: key.kid.clone(),
        payload: payload_bytes,
    })
}

fn select<'a>(keys: &'a JwkSet, algorithm: Algorithm, kid: Option<&str>) -> Result<&'a Jwk> {
    let usable = |key: &&Jwk| {
        key.kty == algorithm.key_type()
            && key.usage.as_deref().is_none_or(|usage| usage == "sig")
            && key.alg.as_deref().is_none_or(|alg| alg == algorithm.name())
    };
    let mut candidates = keys.keys.iter().filter(usable);
    match kid {
        Some(kid) => candidates
            .find(|key| key.kid.as_deref() == Some(kid))
            .ok_or_else(|| invalid(format!("no published key {kid:?} verifies the token"))),
        None => {
            let first = candidates.next();
            match (first, candidates.next()) {
                (Some(key), None) => Ok(key),
                (None, _) => Err(invalid("no published key verifies the token")),
                (Some(_), Some(_)) => {
                    Err(invalid("the token names no key and several could sign it"))
                }
            }
        }
    }
}

fn component(value: Option<&String>, name: &str) -> Result<Vec<u8>> {
    let value = value.ok_or_else(|| invalid(format!("the key has no {name:?}")))?;
    let bytes = decode(value, name)?;
    if bytes.is_empty() {
        return Err(invalid(format!("the key's {name:?} is empty")));
    }
    Ok(bytes)
}

fn check(key: &Jwk, algorithm: Algorithm, signed: &[u8], signature: &[u8]) -> Result<()> {
    let refused = || invalid("the token's signature does not verify");
    match algorithm {
        Algorithm::Rs256
        | Algorithm::Rs384
        | Algorithm::Rs512
        | Algorithm::Ps256
        | Algorithm::Ps384
        | Algorithm::Ps512 => {
            let parameters: &signature::RsaParameters = match algorithm {
                Algorithm::Rs256 => &signature::RSA_PKCS1_2048_8192_SHA256,
                Algorithm::Rs384 => &signature::RSA_PKCS1_2048_8192_SHA384,
                Algorithm::Rs512 => &signature::RSA_PKCS1_2048_8192_SHA512,
                Algorithm::Ps256 => &signature::RSA_PSS_2048_8192_SHA256,
                Algorithm::Ps384 => &signature::RSA_PSS_2048_8192_SHA384,
                _ => &signature::RSA_PSS_2048_8192_SHA512,
            };
            let n = component(key.n.as_ref(), "n")?;
            let e = component(key.e.as_ref(), "e")?;
            RsaPublicKeyComponents { n: &n, e: &e }
                .verify(parameters, signed, signature)
                .map_err(|_| refused())
        }
        Algorithm::Es256 | Algorithm::Es384 => {
            let (curve, size, verification): (&str, usize, &dyn VerificationAlgorithm) =
                if algorithm == Algorithm::Es256 {
                    ("P-256", 32, &signature::ECDSA_P256_SHA256_FIXED)
                } else {
                    ("P-384", 48, &signature::ECDSA_P384_SHA384_FIXED)
                };
            if key.crv.as_deref() != Some(curve) {
                return Err(invalid(format!("the key is not on {curve}")));
            }
            let x = component(key.x.as_ref(), "x")?;
            let y = component(key.y.as_ref(), "y")?;
            if x.len() != size || y.len() != size {
                return Err(invalid("the key's coordinates have the wrong length"));
            }
            let mut point = Vec::with_capacity(1 + 2 * size);
            point.push(0x04);
            point.extend_from_slice(&x);
            point.extend_from_slice(&y);
            UnparsedPublicKey::new(verification, point)
                .verify(signed, signature)
                .map_err(|_| refused())
        }
        Algorithm::EdDsa => {
            if key.crv.as_deref() != Some("Ed25519") {
                return Err(invalid("only Ed25519 keys sign EdDSA tokens"));
            }
            let x = component(key.x.as_ref(), "x")?;
            UnparsedPublicKey::new(&signature::ED25519, x)
                .verify(signed, signature)
                .map_err(|_| refused())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The examples of RFC 7515 Appendix A.2 and A.3 and RFC 8037 A.4.
    const EXAMPLES: &str = include_str!("../tests/fixtures/jws-examples.json");

    fn example(name: &str) -> (String, JwkSet) {
        let examples: Value = serde_json::from_str(EXAMPLES).unwrap();
        let case = &examples[name];
        let key: Jwk = serde_json::from_value(case["jwk"].clone()).unwrap();
        (
            case["jws"].as_str().unwrap().to_owned(),
            JwkSet { keys: vec![key] },
        )
    }

    #[test]
    fn rfc_examples_verify() {
        for (name, algorithm) in [
            ("rs256", Algorithm::Rs256),
            ("es256", Algorithm::Es256),
            ("eddsa", Algorithm::EdDsa),
        ] {
            let (token, keys) = example(name);
            let verified = verify(&token, &keys, &Algorithm::ALL).unwrap();
            assert_eq!(verified.algorithm, algorithm, "{name}");
            assert!(!verified.payload.is_empty());
        }
        let (token, keys) = example("eddsa");
        assert_eq!(
            verify(&token, &keys, &Algorithm::ALL).unwrap().payload,
            b"Example of Ed25519 signing"
        );
    }

    #[test]
    fn tampering_and_wrong_keys_are_refused() {
        let (token, keys) = example("rs256");
        let mut parts: Vec<&str> = token.split('.').collect();
        let forged_payload = URL_SAFE_NO_PAD.encode(br#"{"iss":"mallory"}"#);
        parts[1] = &forged_payload;
        assert!(verify(&parts.join("."), &keys, &Algorithm::ALL).is_err());
        let (_, other_keys) = example("es256");
        assert!(verify(&token, &other_keys, &Algorithm::ALL).is_err());
        assert!(
            verify(&token, &keys, &[Algorithm::Es256]).is_err(),
            "algorithms outside the allowed list are refused"
        );
        for header in [
            r#"{"alg":"none"}"#,
            r#"{"alg":"HS256"}"#,
            r#"{"alg":"RS256","crit":["exp"]}"#,
        ] {
            let forged = format!(
                "{}.{}.{}",
                URL_SAFE_NO_PAD.encode(header),
                token.split('.').nth(1).unwrap(),
                token.split('.').nth(2).unwrap()
            );
            assert!(verify(&forged, &keys, &Algorithm::ALL).is_err(), "{header}");
        }
        assert!(verify("a.b", &keys, &Algorithm::ALL).is_err());
        assert!(verify(&format!("{token}.extra"), &keys, &Algorithm::ALL).is_err());
    }

    #[test]
    fn keys_are_chosen_by_kid_type_use_and_algorithm() {
        let (token, keys) = example("rs256");
        let mut key = keys.keys[0].clone();
        key.kid = Some("one".into());
        let mut encryption = key.clone();
        encryption.kid = Some("two".into());
        encryption.usage = Some("enc".into());
        let set = JwkSet {
            keys: vec![encryption.clone(), key.clone()],
        };
        assert_eq!(
            verify(&token, &set, &Algorithm::ALL)
                .unwrap()
                .key_id
                .as_deref(),
            Some("one"),
            "a token without kid uses the one key able to verify it"
        );
        let ambiguous = JwkSet {
            keys: vec![key.clone(), key.clone()],
        };
        assert!(verify(&token, &ambiguous, &Algorithm::ALL).is_err());
        let mut pinned = key;
        pinned.alg = Some("PS256".into());
        assert!(verify(&token, &JwkSet { keys: vec![pinned] }, &Algorithm::ALL).is_err());
    }

    fn compact(algorithm: Algorithm, sign: impl Fn(&[u8]) -> Vec<u8>) -> String {
        let header = URL_SAFE_NO_PAD.encode(format!(r#"{{"alg":"{}"}}"#, algorithm.name()));
        let payload = URL_SAFE_NO_PAD.encode(br#"{"sub":"alice"}"#);
        let signed = format!("{header}.{payload}");
        format!(
            "{signed}.{}",
            URL_SAFE_NO_PAD.encode(sign(signed.as_bytes()))
        )
    }

    /// RSA with a key OpenSSL generated, signed by ring.
    #[test]
    fn rsa_pss_and_larger_hashes_verify() {
        use base64::engine::general_purpose::STANDARD;
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/rsa-test-key.json")).unwrap();
        let pkcs8 = STANDARD.decode(fixture["pkcs8"].as_str().unwrap()).unwrap();
        let pair = signature::RsaKeyPair::from_pkcs8(&pkcs8).unwrap();
        let keys = JwkSet {
            keys: vec![Jwk {
                kty: "RSA".into(),
                n: Some(fixture["n"].as_str().unwrap().into()),
                e: Some("AQAB".into()),
                ..Jwk::default()
            }],
        };
        let random = ring::rand::SystemRandom::new();
        let encodings: [(Algorithm, &'static dyn signature::RsaEncoding); 5] = [
            (Algorithm::Rs384, &signature::RSA_PKCS1_SHA384),
            (Algorithm::Rs512, &signature::RSA_PKCS1_SHA512),
            (Algorithm::Ps256, &signature::RSA_PSS_SHA256),
            (Algorithm::Ps384, &signature::RSA_PSS_SHA384),
            (Algorithm::Ps512, &signature::RSA_PSS_SHA512),
        ];
        for (algorithm, encoding) in encodings {
            let token = compact(algorithm, |message| {
                let mut signature = vec![0; pair.public().modulus_len()];
                pair.sign(encoding, &random, message, &mut signature)
                    .unwrap();
                signature
            });
            assert_eq!(
                verify(&token, &keys, &Algorithm::ALL).unwrap().algorithm,
                algorithm
            );
        }
    }

    #[test]
    fn p384_signatures_verify() {
        use signature::KeyPair;
        let random = ring::rand::SystemRandom::new();
        let pkcs8 = signature::EcdsaKeyPair::generate_pkcs8(
            &signature::ECDSA_P384_SHA384_FIXED_SIGNING,
            &random,
        )
        .unwrap();
        let pair = signature::EcdsaKeyPair::from_pkcs8(
            &signature::ECDSA_P384_SHA384_FIXED_SIGNING,
            pkcs8.as_ref(),
            &random,
        )
        .unwrap();
        let point = pair.public_key().as_ref();
        let keys = JwkSet {
            keys: vec![Jwk {
                kty: "EC".into(),
                crv: Some("P-384".into()),
                x: Some(URL_SAFE_NO_PAD.encode(&point[1..49])),
                y: Some(URL_SAFE_NO_PAD.encode(&point[49..])),
                ..Jwk::default()
            }],
        };
        let token = compact(Algorithm::Es384, |message| {
            pair.sign(&random, message).unwrap().as_ref().to_vec()
        });
        assert_eq!(
            verify(&token, &keys, &Algorithm::ALL).unwrap().algorithm,
            Algorithm::Es384
        );
        let mut wrong_curve = keys.keys[0].clone();
        wrong_curve.crv = Some("P-256".into());
        let wrong = JwkSet {
            keys: vec![wrong_curve],
        };
        assert!(verify(&token, &wrong, &Algorithm::ALL).is_err());
    }
}
