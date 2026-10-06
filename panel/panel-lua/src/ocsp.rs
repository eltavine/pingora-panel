//! OCSP (RFC 6960) as `ngx.ocsp` speaks it: the responder a certificate
//! names, the request that asks it about the certificate, and whether its
//! response vouches for the certificate now.

use crate::x509::{bits, element, elements, tlv, Certificate, Element};
use sha1::{Digest, Sha1};
use sha2::Sha256;

/// `id-sha1`, with its tag, as CertIDs name it.
const SHA1: &[u8] = &[0x06, 0x05, 0x2b, 0x0e, 0x03, 0x02, 0x1a];
/// `id-sha256`, with its tag.
const SHA256: &[u8] = &[
    0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01,
];
const AUTHORITY_INFO_ACCESS: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x01, 0x01];
const OCSP_ACCESS: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x30, 0x01];
const OCSP_BASIC: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x30, 0x01, 0x01];
const EXTENDED_KEY_USAGE: &[u8] = &[0x55, 0x1d, 0x25];
const OCSP_SIGNING: &[u8] = &[0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x03, 0x09];
/// How far clocks may disagree, as OpenSSL's `OCSP_check_validity` lets
/// them.
const SKEW: i64 = 300;

impl<'a> Certificate<'a> {
    /// The OCSP responder its authority information access names.
    fn responder(&self) -> Option<&'a [u8]> {
        let (access, _) = element(self.extension(AUTHORITY_INFO_ACCESS)?)?;
        elements(access.content)?
            .into_iter()
            .find_map(|description| {
                let parts = elements(description.content)?;
                match parts.as_slice() {
                    [method, location]
                        if method.tag == 0x06
                            && method.content == OCSP_ACCESS
                            && location.tag == 0x86 =>
                    {
                        Some(location.content)
                    }
                    _ => None,
                }
            })
    }

    /// Whether it may sign OCSP responses for its issuer.
    fn signs_ocsp(&self) -> bool {
        self.extension(EXTENDED_KEY_USAGE)
            .and_then(|usages| element(usages))
            .and_then(|(usages, _)| elements(usages.content))
            .is_some_and(|usages| {
                usages
                    .iter()
                    .any(|usage| usage.tag == 0x06 && usage.content == OCSP_SIGNING)
            })
    }
}

/// Whether `signer` made `signature` over `message` with `algorithm`.
fn signed(algorithm: &[u8], signer: &Certificate, message: &[u8], signature: &[u8]) -> bool {
    let provider = rustls::crypto::ring::default_provider();
    provider
        .signature_verification_algorithms
        .all
        .iter()
        .any(|candidate| {
            candidate.signature_alg_id().as_ref() == algorithm
                && candidate.public_key_alg_id().as_ref() == signer.key_algorithm
                && candidate
                    .verify_signature(signer.key, message, signature)
                    .is_ok()
        })
}

fn digest(algorithm: &[u8], data: &[u8]) -> Option<Vec<u8>> {
    match algorithm {
        SHA1 => Some(Sha1::digest(data).to_vec()),
        SHA256 => Some(Sha256::digest(data).to_vec()),
        _ => None,
    }
}

/// The leaf of `chain` and its issuer.
fn leaf_and_issuer(chain: &[Vec<u8>]) -> Result<(Certificate<'_>, Certificate<'_>), String> {
    let leaf = chain
        .first()
        .and_then(|leaf| Certificate::parse(leaf))
        .ok_or("the certificate chain holds no certificate")?;
    let issuer = chain
        .get(1)
        .and_then(|issuer| Certificate::parse(issuer))
        .ok_or("no issuer certificate in chain")?;
    Ok((leaf, issuer))
}

/// The OCSP responder the leaf of `chain` names.
pub(crate) fn responder(chain: &[Vec<u8>]) -> Result<String, String> {
    let leaf = chain
        .first()
        .and_then(|leaf| Certificate::parse(leaf))
        .ok_or("the certificate chain holds no certificate")?;
    let url = leaf
        .responder()
        .ok_or("no OCSP responder URL in the certificate")?;
    String::from_utf8(url.to_vec()).map_err(|_| "the OCSP responder URL is not text".to_owned())
}

/// The request that asks about the leaf of `chain`.
pub(crate) fn request(chain: &[Vec<u8>]) -> Result<Vec<u8>, String> {
    let (leaf, issuer) = leaf_and_issuer(chain)?;
    let algorithm = tlv(0x30, &[SHA1, &[0x05, 0x00][..]].concat());
    let id = [
        algorithm,
        tlv(0x04, &Sha1::digest(leaf.issuer)),
        tlv(0x04, &Sha1::digest(issuer.key)),
        tlv(0x02, leaf.serial),
    ]
    .concat();
    let request = tlv(0x30, &tlv(0x30, &id));
    Ok(tlv(0x30, &tlv(0x30, &tlv(0x30, &request))))
}

/// Seconds since the epoch at a GeneralizedTime such as `20260102030405Z`.
fn generalized_time(element: &Element) -> Option<i64> {
    let text = std::str::from_utf8(element.content).ok()?;
    let digits = text.strip_suffix('Z')?;
    if element.tag != 0x18 || digits.len() < 14 || !digits[..14].bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let number = |range: std::ops::Range<usize>| digits[range].parse::<i64>().ok();
    let (year, month, day) = (number(0..4)?, number(4..6)?, number(6..8)?);
    let (hour, minute, second) = (number(8..10)?, number(10..12)?, number(12..14)?);
    // Days from civil, Howard Hinnant's algorithm.
    let shifted = if month <= 2 { year - 1 } else { year };
    let era = shifted.div_euclid(400);
    let of_era = shifted - era * 400;
    let of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let of_cycle = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
    let days = era * 146_097 + of_cycle - 719_468;
    Some(days * 86_400 + hour * 3_600 + minute * 60 + second)
}

/// Whether `response`, a DER OCSPResponse, says the leaf of `chain` is
/// good at `now`, in seconds since the epoch, signed by its issuer or by a
/// responder the issuer named.
pub(crate) fn validate(response: &[u8], chain: &[Vec<u8>], now: i64) -> Result<(), String> {
    let malformed = || "the OCSP response is malformed".to_owned();
    let (leaf, issuer) = leaf_and_issuer(chain)?;
    let (outer, _) = element(response).ok_or_else(malformed)?;
    let parts = elements(outer.content).ok_or_else(malformed)?;
    let (status, bytes) = match parts.as_slice() {
        [status, bytes, ..] if status.tag == 0x0a && bytes.tag == 0xa0 => (status, bytes),
        [status, ..] if status.tag == 0x0a => {
            return Err(format!(
                "OCSP response not successful ({})",
                status.content.first().copied().unwrap_or(0)
            ))
        }
        _ => return Err(malformed()),
    };
    if status.content != [0] {
        return Err(format!(
            "OCSP response not successful ({})",
            status.content.first().copied().unwrap_or(0)
        ));
    }
    let (wrapped, _) = element(bytes.content).ok_or_else(malformed)?;
    let wrapped = elements(wrapped.content).ok_or_else(malformed)?;
    let [kind, basic] = wrapped.as_slice() else {
        return Err(malformed());
    };
    if kind.content != OCSP_BASIC || basic.tag != 0x04 {
        return Err("the OCSP response is not a basic one".into());
    }
    let (basic, _) = element(basic.content).ok_or_else(malformed)?;
    let basic = elements(basic.content).ok_or_else(malformed)?;
    let [data, algorithm, signature, rest @ ..] = basic.as_slice() else {
        return Err(malformed());
    };
    let signature = bits(signature).ok_or_else(malformed)?;
    let included: Vec<&[u8]> = match rest.first() {
        Some(certs) if certs.tag == 0xa0 => {
            let (certs, _) = element(certs.content).ok_or_else(malformed)?;
            elements(certs.content)
                .ok_or_else(malformed)?
                .into_iter()
                .map(|cert| cert.whole)
                .collect()
        }
        _ => Vec::new(),
    };
    let mut fields = elements(data.content)
        .ok_or_else(malformed)?
        .into_iter()
        .peekable();
    if fields.peek().is_some_and(|field| field.tag == 0xa0) {
        fields.next();
    }
    let responder = fields.next().ok_or_else(malformed)?;
    let _produced = fields.next().ok_or_else(malformed)?;
    let responses = fields.next().ok_or_else(malformed)?;
    let names = |candidate: &Certificate| match responder.tag {
        0xa1 => responder.content == candidate.subject,
        0xa2 => element(responder.content)
            .is_some_and(|(hash, _)| hash.content == Sha1::digest(candidate.key).as_slice()),
        _ => false,
    };
    let delegated: Vec<Certificate> = included
        .iter()
        .filter_map(|cert| Certificate::parse(cert))
        .collect();
    let signer = if names(&issuer) {
        &issuer
    } else {
        let delegate = delegated
            .iter()
            .find(|candidate| names(candidate))
            .ok_or("the OCSP response's signer is not the issuer or a responder it names")?;
        if delegate.issuer != issuer.subject
            || !delegate.signs_ocsp()
            || !signed(
                delegate.signature_algorithm,
                &issuer,
                delegate.tbs,
                delegate.signature,
            )
        {
            return Err("the OCSP responder's certificate is not the issuer's for OCSP".into());
        }
        delegate
    };
    if !signed(algorithm.content, signer, data.whole, signature) {
        return Err("the OCSP response's signature does not verify".into());
    }
    for single in elements(responses.content).ok_or_else(malformed)? {
        let parts = elements(single.content).ok_or_else(malformed)?;
        let [id, status, this_update, rest @ ..] = parts.as_slice() else {
            return Err(malformed());
        };
        let id = elements(id.content).ok_or_else(malformed)?;
        let [hash, name_hash, key_hash, serial] = id.as_slice() else {
            return Err(malformed());
        };
        let (hash, _) = element(hash.content).ok_or_else(malformed)?;
        let Some(name) = digest(hash.whole, leaf.issuer) else {
            continue;
        };
        let key = digest(hash.whole, issuer.key).unwrap_or_default();
        if name_hash.content != name || key_hash.content != key || serial.content != leaf.serial {
            continue;
        }
        match status.tag {
            0x80 => {}
            0xa1 => return Err("certificate status \"revoked\" in the OCSP response".into()),
            _ => return Err("certificate status \"unknown\" in the OCSP response".into()),
        }
        let this_update = generalized_time(this_update).ok_or_else(malformed)?;
        if this_update > now + SKEW {
            return Err("the OCSP response is not yet valid".into());
        }
        if let Some(next) = rest.iter().find(|field| field.tag == 0xa0) {
            let (next, _) = element(next.content).ok_or_else(malformed)?;
            if generalized_time(&next).ok_or_else(malformed)? < now - SKEW {
                return Err("the OCSP response has expired".into());
            }
        }
        return Ok(());
    }
    Err("could not find the certificate status in the OCSP response".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{
        BasicConstraints, CertificateParams, CustomExtension, IsCa, Issuer, KeyPair, SigningKey,
    };

    const ECDSA_SHA256: &[u8] = &[0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];

    fn time(text: &str) -> Vec<u8> {
        tlv(0x18, text.as_bytes())
    }

    /// A response about `serial` of `issuer`'s, signed with `key`.
    fn response(chain: &[Vec<u8>], key: &KeyPair, status: &[u8], next: &str) -> Vec<u8> {
        let (leaf, issuer) = leaf_and_issuer(chain).unwrap();
        let algorithm = tlv(0x30, &[SHA1, &[0x05, 0x00][..]].concat());
        let id = tlv(
            0x30,
            &[
                algorithm,
                tlv(0x04, &Sha1::digest(leaf.issuer)),
                tlv(0x04, &Sha1::digest(issuer.key)),
                tlv(0x02, leaf.serial),
            ]
            .concat(),
        );
        let single = tlv(
            0x30,
            &[
                id,
                status.to_vec(),
                time("20260101000000Z"),
                tlv(0xa0, &time(next)),
            ]
            .concat(),
        );
        let data = tlv(
            0x30,
            &[
                tlv(0xa2, &tlv(0x04, &Sha1::digest(issuer.key))),
                time("20260101000000Z"),
                tlv(0x30, &single),
            ]
            .concat(),
        );
        let signature = key.sign(&data).unwrap();
        let mut signature_bits = vec![0];
        signature_bits.extend(signature);
        let basic = tlv(
            0x30,
            &[data, tlv(0x30, ECDSA_SHA256), tlv(0x03, &signature_bits)].concat(),
        );
        let bytes = tlv(0x30, &[tlv(0x06, OCSP_BASIC), tlv(0x04, &basic)].concat());
        tlv(0x30, &[tlv(0x0a, &[0]), tlv(0xa0, &bytes)].concat())
    }

    #[test]
    fn certificates_give_their_responder_and_responses_are_checked() {
        let ca_key = KeyPair::generate().unwrap();
        let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ca_cert = ca.self_signed(&ca_key).unwrap();
        let issuer = Issuer::from_params(&ca, &ca_key);
        let leaf_key = KeyPair::generate().unwrap();
        let mut leaf = CertificateParams::new(vec!["shop.example".into()]).unwrap();
        let access = tlv(
            0x30,
            &tlv(
                0x30,
                &[tlv(0x06, OCSP_ACCESS), tlv(0x86, b"http://ocsp.example/")].concat(),
            ),
        );
        leaf.custom_extensions = vec![CustomExtension::from_oid_content(
            &[1, 3, 6, 1, 5, 5, 7, 1, 1],
            access,
        )];
        let leaf_cert = leaf.signed_by(&leaf_key, &issuer).unwrap();
        let chain = vec![leaf_cert.der().to_vec(), ca_cert.der().to_vec()];

        assert_eq!(responder(&chain).unwrap(), "http://ocsp.example/");
        assert!(responder(&chain[1..]).is_err());
        let asked = request(&chain).unwrap();
        assert_eq!(asked[0], 0x30);
        assert!(asked
            .windows(20)
            .any(|window| window
                == Sha1::digest(Certificate::parse(&chain[1]).unwrap().key).as_slice()));
        assert_eq!(
            request(&chain[..1]).unwrap_err(),
            "no issuer certificate in chain"
        );

        let now = generalized_time(&element(&time("20260102000000Z")).unwrap().0).unwrap();
        let good = response(&chain, &ca_key, &[0x80, 0x00], "20260108000000Z");
        assert_eq!(validate(&good, &chain, now), Ok(()));
        let later = now + 30 * 86_400;
        assert_eq!(
            validate(&good, &chain, later).unwrap_err(),
            "the OCSP response has expired"
        );
        let revoked = response(
            &chain,
            &ca_key,
            &tlv(0xa1, &time("20251201000000Z")),
            "20260108000000Z",
        );
        assert!(validate(&revoked, &chain, now)
            .unwrap_err()
            .contains("revoked"));
        let forged = response(&chain, &leaf_key, &[0x80, 0x00], "20260108000000Z");
        assert!(validate(&forged, &chain, now)
            .unwrap_err()
            .contains("signature"));
        assert!(validate(b"\x30\x03\x0a\x01\x06", &chain, now)
            .unwrap_err()
            .contains("not successful"));
        assert_eq!(
            generalized_time(&element(&time("19700101000001Z")).unwrap().0),
            Some(1)
        );
    }
}
