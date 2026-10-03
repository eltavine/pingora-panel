//! JSON Web Tokens (RFC 7519) through `jsonwebtoken`, with `ring` doing the
//! cryptography so the panel keeps one stack. Only the asymmetric algorithms
//! of RFC 7518 and EdDSA (RFC 8037) are accepted: the panel verifies what a
//! provider signed with a key it published, never `none` or a shared secret.

use jsonwebtoken::{
    crypto::{CryptoProvider, JwtSigner, JwtVerifier, KeyUtils},
    decode, decode_header,
    errors::{Error, ErrorKind},
    jwk::{AlgorithmParameters, EllipticCurve, Jwk, JwkSet, KeyOperations, PublicKeyUse},
    signature::{self as traits, Signer, Verifier},
    Algorithm, DecodingKey, DecodingKeyKind, EncodingKey, Validation,
};
use ring::{
    rand::SystemRandom,
    signature::{
        self, EcdsaKeyPair, Ed25519KeyPair, RsaEncoding, RsaKeyPair, RsaParameters,
        RsaPublicKeyComponents, UnparsedPublicKey, VerificationAlgorithm,
    },
};
use serde_json::{Map, Value};

/// The algorithms a provider may sign with.
pub(crate) const ACCEPTED: [Algorithm; 9] = [
    Algorithm::RS256,
    Algorithm::RS384,
    Algorithm::RS512,
    Algorithm::PS256,
    Algorithm::PS384,
    Algorithm::PS512,
    Algorithm::ES256,
    Algorithm::ES384,
    Algorithm::EdDSA,
];

static RING: CryptoProvider = CryptoProvider {
    signer_factory: signer,
    verifier_factory: verifier,
    key_utils: KeyUtils::new_unimplemented(),
};

/// Makes `ring` the process's JOSE cryptography before its first use; a
/// provider installed earlier stays.
fn install() {
    let _ = RING.install_default();
}

/// Why a token was refused, as a phrase about it.
#[derive(Debug)]
pub(crate) struct Refusal {
    phrase: String,
    keys: bool,
}

impl Refusal {
    fn new(phrase: impl Into<String>) -> Self {
        Self {
            phrase: phrase.into(),
            keys: false,
        }
    }

    fn keys(phrase: impl Into<String>) -> Self {
        Self {
            phrase: phrase.into(),
            keys: true,
        }
    }

    /// Whether newer keys might accept the token.
    pub(crate) fn concerns_keys(&self) -> bool {
        self.keys
    }

    pub(crate) fn phrase(&self) -> &str {
        &self.phrase
    }
}

impl From<Error> for Refusal {
    fn from(error: Error) -> Self {
        match error.kind() {
            ErrorKind::InvalidSignature => Self::keys("has a signature that does not verify"),
            ErrorKind::ExpiredSignature => Self::new("has expired"),
            ErrorKind::ImmatureSignature => Self::new("is not valid yet"),
            ErrorKind::InvalidIssuer => Self::new("comes from another issuer"),
            ErrorKind::InvalidAudience => Self::new("is meant for another client"),
            ErrorKind::MissingRequiredClaim(claim) => Self::new(format!("has no {claim:?} claim")),
            ErrorKind::InvalidAlgorithm | ErrorKind::InvalidAlgorithmName => {
                Self::new("uses an algorithm that is not accepted")
            }
            _ => Self::new("is not a signed JWT"),
        }
    }
}

/// The claims of `token` once a key in `keys` verified it with one of the
/// `allowed` algorithms and `validation` accepted its registered claims.
pub(crate) fn verify(
    token: &str,
    keys: &JwkSet,
    allowed: &[Algorithm],
    validation: &Validation,
) -> Result<Map<String, Value>, Refusal> {
    install();
    let header = decode_header(token)?;
    if header.crit.is_some() {
        return Err(Refusal::new("names critical extensions"));
    }
    if !(allowed.contains(&header.alg) && ACCEPTED.contains(&header.alg)) {
        return Err(Refusal::new("uses an algorithm that is not accepted"));
    }
    let key = DecodingKey::from_jwk(select(keys, header.alg, header.kid.as_deref())?)
        .map_err(|_| Refusal::keys("names a key that cannot verify it"))?;
    let mut validation = validation.clone();
    validation.algorithms = vec![header.alg];
    Ok(decode::<Map<String, Value>>(token, &key, &validation)?.claims)
}

/// A token signed with a PKCS #8 key, as test providers sign them.
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn sign(
    header: &jsonwebtoken::Header,
    claims: &Value,
    pkcs8: &[u8],
) -> Result<String, Error> {
    install();
    let key = match header.alg {
        Algorithm::ES256 | Algorithm::ES384 => EncodingKey::from_ec_der(pkcs8),
        Algorithm::EdDSA => EncodingKey::from_ed_der(pkcs8),
        _ => EncodingKey::from_rsa_der(pkcs8),
    };
    jsonwebtoken::encode(header, claims, &key)
}

/// The published key for `algorithm` named `kid`, or the only one when the
/// token names none.
fn select<'a>(
    keys: &'a JwkSet,
    algorithm: Algorithm,
    kid: Option<&str>,
) -> Result<&'a Jwk, Refusal> {
    let mut candidates = keys.keys.iter().filter(|key| fits(key, algorithm));
    match kid {
        Some(kid) => candidates
            .find(|key| key.common.key_id.as_deref() == Some(kid))
            .ok_or_else(|| Refusal::keys(format!("names key {kid:?}, which is not published"))),
        None => match (candidates.next(), candidates.next()) {
            (Some(key), None) => Ok(key),
            (None, _) => Err(Refusal::keys("matches no published key")),
            (Some(_), Some(_)) => Err(Refusal::new("names no key and several could sign it")),
        },
    }
}

/// Whether `key` may verify `algorithm` signatures (RFC 7517 §4).
fn fits(key: &Jwk, algorithm: Algorithm) -> bool {
    let shape = match (&key.algorithm, algorithm) {
        (
            AlgorithmParameters::RSA(_),
            Algorithm::RS256
            | Algorithm::RS384
            | Algorithm::RS512
            | Algorithm::PS256
            | Algorithm::PS384
            | Algorithm::PS512,
        ) => true,
        (AlgorithmParameters::EllipticCurve(key), Algorithm::ES256) => {
            key.curve == EllipticCurve::P256
        }
        (AlgorithmParameters::EllipticCurve(key), Algorithm::ES384) => {
            key.curve == EllipticCurve::P384
        }
        (AlgorithmParameters::OctetKeyPair(key), Algorithm::EdDSA) => {
            key.curve == EllipticCurve::Ed25519
        }
        _ => false,
    };
    let common = &key.common;
    shape
        && common
            .public_key_use
            .as_ref()
            .is_none_or(|usage| *usage == PublicKeyUse::Signature)
        && common
            .key_operations
            .as_ref()
            .is_none_or(|operations| operations.contains(&KeyOperations::Verify))
        && common.key_algorithm.is_none_or(|named| {
            named
                .to_string()
                .parse::<Algorithm>()
                .is_ok_and(|named| named == algorithm)
        })
}

enum PublicKey {
    Rsa {
        parameters: &'static RsaParameters,
        n: Vec<u8>,
        e: Vec<u8>,
    },
    Encoded(&'static dyn VerificationAlgorithm, Vec<u8>),
}

struct RingVerifier {
    algorithm: Algorithm,
    key: PublicKey,
}

fn verifier(algorithm: &Algorithm, key: &DecodingKey) -> Result<Box<dyn JwtVerifier>, Error> {
    let rsa = |parameters| match key.kind() {
        DecodingKeyKind::RsaModulusExponent { n, e } => Ok(PublicKey::Rsa {
            parameters,
            n: n.clone(),
            e: e.clone(),
        }),
        DecodingKeyKind::SecretOrDer(_) => Err(Error::from(ErrorKind::InvalidKeyFormat)),
    };
    let encoded = |verification: &'static dyn VerificationAlgorithm| {
        key.try_get_as_bytes()
            .map(|bytes| PublicKey::Encoded(verification, bytes.to_vec()))
    };
    let key = match algorithm {
        Algorithm::RS256 => rsa(&signature::RSA_PKCS1_2048_8192_SHA256)?,
        Algorithm::RS384 => rsa(&signature::RSA_PKCS1_2048_8192_SHA384)?,
        Algorithm::RS512 => rsa(&signature::RSA_PKCS1_2048_8192_SHA512)?,
        Algorithm::PS256 => rsa(&signature::RSA_PSS_2048_8192_SHA256)?,
        Algorithm::PS384 => rsa(&signature::RSA_PSS_2048_8192_SHA384)?,
        Algorithm::PS512 => rsa(&signature::RSA_PSS_2048_8192_SHA512)?,
        Algorithm::ES256 => encoded(&signature::ECDSA_P256_SHA256_FIXED)?,
        Algorithm::ES384 => encoded(&signature::ECDSA_P384_SHA384_FIXED)?,
        Algorithm::EdDSA => encoded(&signature::ED25519)?,
        _ => return Err(ErrorKind::InvalidAlgorithm.into()),
    };
    Ok(Box::new(RingVerifier {
        algorithm: *algorithm,
        key,
    }))
}

impl Verifier<Vec<u8>> for RingVerifier {
    fn verify(&self, message: &[u8], signature: &Vec<u8>) -> Result<(), traits::Error> {
        match &self.key {
            PublicKey::Rsa { parameters, n, e } => {
                RsaPublicKeyComponents { n, e }.verify(parameters, message, signature)
            }
            PublicKey::Encoded(algorithm, key) => {
                UnparsedPublicKey::new(*algorithm, key).verify(message, signature)
            }
        }
        .map_err(|_| traits::Error::new())
    }
}

impl JwtVerifier for RingVerifier {
    fn algorithm(&self) -> Algorithm {
        self.algorithm
    }
}

enum PrivateKey {
    Rsa(&'static dyn RsaEncoding, RsaKeyPair),
    Ecdsa(EcdsaKeyPair),
    Ed25519(Ed25519KeyPair),
}

struct RingSigner {
    algorithm: Algorithm,
    key: PrivateKey,
}

fn signer(algorithm: &Algorithm, key: &EncodingKey) -> Result<Box<dyn JwtSigner>, Error> {
    let pkcs8 = key.as_bytes();
    let invalid = |_| Error::from(ErrorKind::InvalidKeyFormat);
    let rsa = |encoding| {
        RsaKeyPair::from_pkcs8(pkcs8)
            .map(|pair| PrivateKey::Rsa(encoding, pair))
            .map_err(invalid)
    };
    let ecdsa = |signing| {
        EcdsaKeyPair::from_pkcs8(signing, pkcs8, &SystemRandom::new())
            .map(PrivateKey::Ecdsa)
            .map_err(invalid)
    };
    let key = match algorithm {
        Algorithm::RS256 => rsa(&signature::RSA_PKCS1_SHA256)?,
        Algorithm::RS384 => rsa(&signature::RSA_PKCS1_SHA384)?,
        Algorithm::RS512 => rsa(&signature::RSA_PKCS1_SHA512)?,
        Algorithm::PS256 => rsa(&signature::RSA_PSS_SHA256)?,
        Algorithm::PS384 => rsa(&signature::RSA_PSS_SHA384)?,
        Algorithm::PS512 => rsa(&signature::RSA_PSS_SHA512)?,
        Algorithm::ES256 => ecdsa(&signature::ECDSA_P256_SHA256_FIXED_SIGNING)?,
        Algorithm::ES384 => ecdsa(&signature::ECDSA_P384_SHA384_FIXED_SIGNING)?,
        Algorithm::EdDSA => Ed25519KeyPair::from_pkcs8_maybe_unchecked(pkcs8)
            .map(PrivateKey::Ed25519)
            .map_err(invalid)?,
        _ => return Err(ErrorKind::InvalidAlgorithm.into()),
    };
    Ok(Box::new(RingSigner {
        algorithm: *algorithm,
        key,
    }))
}

impl Signer<Vec<u8>> for RingSigner {
    fn try_sign(&self, message: &[u8]) -> Result<Vec<u8>, traits::Error> {
        let random = SystemRandom::new();
        match &self.key {
            PrivateKey::Rsa(encoding, pair) => {
                let mut signature = vec![0; pair.public().modulus_len()];
                pair.sign(*encoding, &random, message, &mut signature)
                    .map(|()| signature)
                    .map_err(|_| traits::Error::new())
            }
            PrivateKey::Ecdsa(pair) => pair
                .sign(&random, message)
                .map(|signature| signature.as_ref().to_vec())
                .map_err(|_| traits::Error::new()),
            PrivateKey::Ed25519(pair) => Ok(pair.sign(message).as_ref().to_vec()),
        }
    }
}

impl JwtSigner for RingSigner {
    fn algorithm(&self) -> Algorithm {
        self.algorithm
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{
        engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
        Engine,
    };
    use jsonwebtoken::{get_current_timestamp, Header};
    use ring::signature::KeyPair;
    use serde_json::json;

    /// The examples of RFC 7515 Appendix A.2 and A.3 and RFC 8037 A.4.
    const EXAMPLES: &str = include_str!("../tests/fixtures/jws-examples.json");

    fn example(name: &str) -> (String, Jwk) {
        let examples: Value = serde_json::from_str(EXAMPLES).unwrap();
        let case = &examples[name];
        (
            case["jws"].as_str().unwrap().to_owned(),
            serde_json::from_value(case["jwk"].clone()).unwrap(),
        )
    }

    fn signature_verifies(token: &str, key: &Jwk) -> bool {
        install();
        let (message, signature) = token.rsplit_once('.').unwrap();
        let algorithm = decode_header(token).unwrap().alg;
        jsonwebtoken::crypto::verify(
            signature,
            message.as_bytes(),
            &DecodingKey::from_jwk(key).unwrap(),
            algorithm,
        )
        .unwrap_or(false)
    }

    fn lenient() -> Validation {
        let mut validation = Validation::new(Algorithm::ES256);
        validation.validate_exp = false;
        validation.validate_aud = false;
        validation.required_spec_claims.clear();
        validation
    }

    #[test]
    fn rfc_examples_verify_with_ring() {
        for name in ["rs256", "es256", "eddsa"] {
            let (token, key) = example(name);
            assert!(signature_verifies(&token, &key), "{name}");
            let mut forged = token.clone();
            forged.replace_range(forged.len() - 4.., "AAAA");
            assert!(!signature_verifies(&forged, &key), "{name}");
        }
        let (token, key) = example("rs256");
        let keys = JwkSet { keys: vec![key] };
        let claims = verify(&token, &keys, &ACCEPTED, &lenient()).unwrap();
        assert_eq!(claims["iss"], "joe");
    }

    #[test]
    fn tampering_and_wrong_keys_are_refused() {
        let (token, key) = example("rs256");
        let keys = JwkSet { keys: vec![key] };
        let mut parts: Vec<&str> = token.split('.').collect();
        let forged_payload = URL_SAFE_NO_PAD.encode(br#"{"iss":"mallory"}"#);
        parts[1] = &forged_payload;
        assert!(verify(&parts.join("."), &keys, &ACCEPTED, &lenient()).is_err());
        let (_, other) = example("es256");
        let refusal =
            verify(&token, &JwkSet { keys: vec![other] }, &ACCEPTED, &lenient()).unwrap_err();
        assert!(refusal.concerns_keys(), "{refusal:?}");
        assert!(
            verify(&token, &keys, &[Algorithm::ES256], &lenient()).is_err(),
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
            assert!(
                verify(&forged, &keys, &ACCEPTED, &lenient()).is_err(),
                "{header}"
            );
        }
        assert!(verify("a.b", &keys, &ACCEPTED, &lenient()).is_err());
        assert!(verify(&format!("{token}.extra"), &keys, &ACCEPTED, &lenient()).is_err());
    }

    #[test]
    fn keys_are_chosen_by_kid_type_use_and_algorithm() {
        let (token, mut key) = example("rs256");
        key.common.key_id = Some("one".into());
        let mut encryption = key.clone();
        encryption.common.key_id = Some("two".into());
        encryption.common.public_key_use = Some(PublicKeyUse::Encryption);
        let keys = JwkSet {
            keys: vec![encryption, key.clone()],
        };
        assert!(
            verify(&token, &keys, &ACCEPTED, &lenient()).is_ok(),
            "a token without kid uses the one key able to verify it"
        );
        let ambiguous = JwkSet {
            keys: vec![key.clone(), key.clone()],
        };
        assert!(verify(&token, &ambiguous, &ACCEPTED, &lenient()).is_err());
        let mut pinned = key;
        pinned.common.key_algorithm = Some(jsonwebtoken::jwk::KeyAlgorithm::PS256);
        assert!(verify(
            &token,
            &JwkSet { keys: vec![pinned] },
            &ACCEPTED,
            &lenient()
        )
        .is_err());
    }

    /// RSA with a key OpenSSL generated, and EC and EdDSA keys ring
    /// generates, signed and verified through the same provider.
    #[test]
    fn every_accepted_algorithm_round_trips() {
        let fixture: Value =
            serde_json::from_str(include_str!("../tests/fixtures/rsa-test-key.json")).unwrap();
        let rsa = STANDARD.decode(fixture["pkcs8"].as_str().unwrap()).unwrap();
        let rsa_key = json!({"kty": "RSA", "n": fixture["n"], "e": "AQAB"});
        let random = SystemRandom::new();
        let ec = |signing, size: usize, curve: &str| {
            let pkcs8 = EcdsaKeyPair::generate_pkcs8(signing, &random).unwrap();
            let pair = EcdsaKeyPair::from_pkcs8(signing, pkcs8.as_ref(), &random).unwrap();
            let point = pair.public_key().as_ref();
            (
                pkcs8.as_ref().to_vec(),
                json!({"kty": "EC", "crv": curve,
                    "x": URL_SAFE_NO_PAD.encode(&point[1..=size]),
                    "y": URL_SAFE_NO_PAD.encode(&point[size + 1..])}),
            )
        };
        let (p256, p256_key) = ec(&signature::ECDSA_P256_SHA256_FIXED_SIGNING, 32, "P-256");
        let (p384, p384_key) = ec(&signature::ECDSA_P384_SHA384_FIXED_SIGNING, 48, "P-384");
        let ed = Ed25519KeyPair::generate_pkcs8(&random).unwrap();
        let ed_key = json!({"kty": "OKP", "crv": "Ed25519", "x": URL_SAFE_NO_PAD.encode(
            Ed25519KeyPair::from_pkcs8(ed.as_ref()).unwrap().public_key().as_ref())});
        let now = get_current_timestamp();
        let claims = json!({"iss": "https://id.example", "sub": "alice", "exp": now + 60});
        let mut validation = Validation::new(Algorithm::ES256);
        validation.validate_aud = false;
        validation.set_issuer(&["https://id.example"]);
        for (algorithm, pkcs8, key) in [
            (Algorithm::RS256, &rsa, &rsa_key),
            (Algorithm::RS384, &rsa, &rsa_key),
            (Algorithm::RS512, &rsa, &rsa_key),
            (Algorithm::PS256, &rsa, &rsa_key),
            (Algorithm::PS384, &rsa, &rsa_key),
            (Algorithm::PS512, &rsa, &rsa_key),
            (Algorithm::ES256, &p256, &p256_key),
            (Algorithm::ES384, &p384, &p384_key),
            (Algorithm::EdDSA, &ed.as_ref().to_vec(), &ed_key),
        ] {
            let token = sign(&Header::new(algorithm), &claims, pkcs8).unwrap();
            let keys = JwkSet {
                keys: vec![serde_json::from_value(key.clone()).unwrap()],
            };
            assert_eq!(
                verify(&token, &keys, &ACCEPTED, &validation)
                    .map(|claims| claims["sub"].clone())
                    .map_err(|refusal| refusal.phrase().to_owned()),
                Ok(json!("alice")),
                "{algorithm:?}"
            );
        }
        let mut wrong_curve = p384_key;
        wrong_curve["crv"] = json!("P-256");
        let token = sign(&Header::new(Algorithm::ES384), &claims, &p384).unwrap();
        let keys = JwkSet {
            keys: vec![serde_json::from_value(wrong_curve).unwrap()],
        };
        assert!(verify(&token, &keys, &ACCEPTED, &validation).is_err());
    }

    #[test]
    fn registered_claims_are_validated() {
        let random = SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&random).unwrap();
        let public = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let keys = JwkSet {
            keys: vec![
                serde_json::from_value(json!({"kty": "OKP", "crv": "Ed25519",
                "x": URL_SAFE_NO_PAD.encode(public.public_key().as_ref())}))
                .unwrap(),
            ],
        };
        let now = get_current_timestamp();
        let mut validation = Validation::new(Algorithm::EdDSA);
        validation.leeway = 60;
        validation.validate_nbf = true;
        validation.set_issuer(&["https://id.example"]);
        validation.set_audience(&["panel"]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        let good = json!({"iss": "https://id.example", "aud": "panel", "sub": "alice",
            "exp": now + 300, "nbf": now});
        let check = |claims: &Value| {
            let token = sign(&Header::new(Algorithm::EdDSA), claims, pkcs8.as_ref()).unwrap();
            verify(&token, &keys, &ACCEPTED, &validation).map_err(|refusal| refusal.phrase)
        };
        assert!(check(&good).is_ok());
        for (field, value, phrase) in [
            (
                "iss",
                json!("https://evil.example"),
                "comes from another issuer",
            ),
            ("aud", json!("other"), "is meant for another client"),
            ("exp", json!(now - 61), "has expired"),
            ("exp", Value::Null, "has no \"exp\" claim"),
            ("nbf", json!(now + 61), "is not valid yet"),
        ] {
            let mut bad = good.clone();
            bad[field] = value.clone();
            assert_eq!(check(&bad).unwrap_err(), phrase, "{field} = {value}");
        }
        let mut late = good;
        late["exp"] = json!(now - 30);
        assert!(check(&late).is_ok(), "a minute of leeway");
    }
}
