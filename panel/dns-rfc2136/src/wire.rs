//! The DNS wire format of dynamic updates (RFC 1035 §4, RFC 2136 §2) and
//! their transaction signatures (RFC 8945 §4).

use hmac::{Hmac, KeyInit, Mac};
use panel_errors::{PanelError, Result};
use sha2::{Sha256, Sha512};
use zeroize::Zeroizing;

const TYPE_SOA: u16 = 6;
const TYPE_TXT: u16 = 16;
const TYPE_TSIG: u16 = 250;
const CLASS_IN: u16 = 1;
const CLASS_NONE: u16 = 254;
const CLASS_ANY: u16 = 255;
const OPCODE_UPDATE: u16 = 5;
const HEADER: usize = 12;
/// Seconds of clock difference a signature tolerates (RFC 8945 §10).
pub(crate) const FUDGE: u16 = 300;

fn malformed(what: &str) -> PanelError {
    PanelError::unavailable(format!("the DNS server sent a malformed answer: {what}"))
}

/// A domain name in uncompressed wire format with lowercase labels, the
/// canonical form RFC 8945 signs.
pub(crate) fn name(text: &str) -> Result<Vec<u8>> {
    let text = text.trim_end_matches('.');
    let mut wire = Vec::with_capacity(text.len() + 2);
    if !text.is_empty() {
        for label in text.split('.') {
            let length = u8::try_from(label.len())
                .ok()
                .filter(|length| (1..=63).contains(length) && label.is_ascii())
                .ok_or_else(|| {
                    PanelError::invalid_argument(format!("{text:?} is not a DNS name"))
                })?;
            wire.push(length);
            wire.extend(label.bytes().map(|byte| byte.to_ascii_lowercase()));
        }
    }
    wire.push(0);
    if wire.len() > 255 {
        return Err(PanelError::invalid_argument(format!(
            "{text:?} is longer than a DNS name may be"
        )));
    }
    Ok(wire)
}

/// Whether an update adds a record or removes it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Change {
    Add,
    Remove,
}

/// An unsigned UPDATE of `zone` that adds or removes the TXT record `value`
/// at `record`; removing names the exact record (RFC 2136 §2.5.4).
pub(crate) fn update(
    id: u16,
    zone: &[u8],
    record: &[u8],
    value: &str,
    ttl: u32,
    change: Change,
) -> Result<Vec<u8>> {
    let text = u8::try_from(value.len())
        .map_err(|_| PanelError::invalid_argument("a TXT string holds at most 255 bytes"))?;
    let (class, ttl) = match change {
        Change::Add => (CLASS_IN, ttl),
        Change::Remove => (CLASS_NONE, 0),
    };
    let mut message = Vec::with_capacity(HEADER + zone.len() + record.len() + value.len() + 64);
    message.extend(id.to_be_bytes());
    message.extend((OPCODE_UPDATE << 11).to_be_bytes());
    for count in [1_u16, 0, 1, 0] {
        message.extend(count.to_be_bytes());
    }
    message.extend(zone);
    message.extend(TYPE_SOA.to_be_bytes());
    message.extend(CLASS_IN.to_be_bytes());
    message.extend(record);
    message.extend(TYPE_TXT.to_be_bytes());
    message.extend(class.to_be_bytes());
    message.extend(ttl.to_be_bytes());
    message.extend((u16::from(text) + 1).to_be_bytes());
    message.push(text);
    message.extend(value.as_bytes());
    Ok(message)
}

/// A TSIG algorithm (RFC 8945 §6).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Algorithm {
    HmacSha256,
    HmacSha512,
}

impl Algorithm {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim_end_matches('.').to_ascii_lowercase().as_str() {
            "hmac-sha256" => Ok(Self::HmacSha256),
            "hmac-sha512" => Ok(Self::HmacSha512),
            _ => Err(PanelError::invalid_argument(format!(
                "{value:?} is not a supported TSIG algorithm: use hmac-sha256 or hmac-sha512"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::HmacSha256 => "hmac-sha256",
            Self::HmacSha512 => "hmac-sha512",
        }
    }

    fn mac(self, secret: &[u8], parts: &[&[u8]]) -> Vec<u8> {
        fn compute<M: Mac + KeyInit>(secret: &[u8], parts: &[&[u8]]) -> Vec<u8> {
            let mut mac =
                <M as KeyInit>::new_from_slice(secret).expect("HMAC takes keys of any length");
            for part in parts {
                mac.update(part);
            }
            mac.finalize().into_bytes().to_vec()
        }
        match self {
            Self::HmacSha256 => compute::<Hmac<Sha256>>(secret, parts),
            Self::HmacSha512 => compute::<Hmac<Sha512>>(secret, parts),
        }
    }

    fn verify(self, secret: &[u8], parts: &[&[u8]], mac: &[u8]) -> bool {
        fn check<M: Mac + KeyInit>(secret: &[u8], parts: &[&[u8]], expected: &[u8]) -> bool {
            let mut mac =
                <M as KeyInit>::new_from_slice(secret).expect("HMAC takes keys of any length");
            for part in parts {
                mac.update(part);
            }
            mac.verify_slice(expected).is_ok()
        }
        match self {
            Self::HmacSha256 => check::<Hmac<Sha256>>(secret, parts, mac),
            Self::HmacSha512 => check::<Hmac<Sha512>>(secret, parts, mac),
        }
    }
}

/// A TSIG key: its name, algorithm and shared secret.
#[derive(Clone)]
pub(crate) struct Key {
    pub name: Vec<u8>,
    pub algorithm: Algorithm,
    pub secret: Zeroizing<Vec<u8>>,
}

/// The TSIG variables a MAC covers after the message (RFC 8945 §4.3.3).
fn variables(key: &Key, time: u64, fudge: u16, error: u16, other: &[u8]) -> Result<Vec<u8>> {
    let algorithm = name(key.algorithm.as_str())?;
    let mut variables = Vec::with_capacity(key.name.len() + algorithm.len() + 24 + other.len());
    variables.extend(&key.name);
    variables.extend(CLASS_ANY.to_be_bytes());
    variables.extend(0_u32.to_be_bytes());
    variables.extend(algorithm);
    variables.extend(&time.to_be_bytes()[2..]);
    variables.extend(fudge.to_be_bytes());
    variables.extend(error.to_be_bytes());
    variables.extend(u16::try_from(other.len()).unwrap_or(0).to_be_bytes());
    variables.extend(other);
    Ok(variables)
}

/// Signs `message` with `key` at `time`, appending its TSIG record, and
/// returns the MAC, which the signature of the answer covers.
pub(crate) fn sign(message: &mut Vec<u8>, key: &Key, time: u64) -> Result<Vec<u8>> {
    let id = [message[0], message[1]];
    let mac = key.algorithm.mac(
        &key.secret,
        &[message, &variables(key, time, FUDGE, 0, &[])?],
    );
    let mut data = name(key.algorithm.as_str())?;
    data.extend(&time.to_be_bytes()[2..]);
    data.extend(FUDGE.to_be_bytes());
    data.extend(u16::try_from(mac.len()).unwrap_or(0).to_be_bytes());
    data.extend(&mac);
    data.extend(id);
    data.extend(0_u16.to_be_bytes());
    data.extend(0_u16.to_be_bytes());
    message.extend(&key.name);
    message.extend(TYPE_TSIG.to_be_bytes());
    message.extend(CLASS_ANY.to_be_bytes());
    message.extend(0_u32.to_be_bytes());
    message.extend(u16::try_from(data.len()).unwrap_or(0).to_be_bytes());
    message.extend(data);
    let records = u16::from_be_bytes([message[10], message[11]]) + 1;
    message[10..12].copy_from_slice(&records.to_be_bytes());
    Ok(mac)
}

/// A message's TSIG record.
#[derive(Debug)]
pub(crate) struct Signature {
    /// Where the record starts in the message.
    start: usize,
    key_name: Vec<u8>,
    algorithm: Vec<u8>,
    time: u64,
    fudge: u16,
    mac: Vec<u8>,
    original_id: u16,
    pub error: u16,
    other: Vec<u8>,
}

/// What an answer says: its ID, response code and signature.
#[derive(Debug)]
pub(crate) struct Answer {
    pub id: u16,
    pub rcode: u16,
    pub signature: Option<Signature>,
}

struct Reader<'a> {
    message: &'a [u8],
    position: usize,
}

impl Reader<'_> {
    fn take(&mut self, count: usize) -> Result<&[u8]> {
        let end = self
            .position
            .checked_add(count)
            .filter(|end| *end <= self.message.len())
            .ok_or_else(|| malformed("it ends too early"))?;
        let bytes = &self.message[self.position..end];
        self.position = end;
        Ok(bytes)
    }

    fn u16(&mut self) -> Result<u16> {
        let bytes = self.take(2)?;
        Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
    }

    /// A name, lowercased, following compression pointers (RFC 1035 §4.1.4).
    fn name(&mut self) -> Result<Vec<u8>> {
        let mut wire = Vec::new();
        let mut position = self.position;
        let mut jumped = false;
        for _ in 0..128 {
            let length = *self
                .message
                .get(position)
                .ok_or_else(|| malformed("a name ends too early"))?;
            match length {
                0 => {
                    wire.push(0);
                    if !jumped {
                        self.position = position + 1;
                    }
                    return Ok(wire);
                }
                1..=63 => {
                    let label = self
                        .message
                        .get(position + 1..position + 1 + usize::from(length))
                        .ok_or_else(|| malformed("a label ends too early"))?;
                    wire.push(length);
                    wire.extend(label.iter().map(u8::to_ascii_lowercase));
                    position += 1 + usize::from(length);
                }
                0xC0..=0xFF => {
                    let low = *self
                        .message
                        .get(position + 1)
                        .ok_or_else(|| malformed("a pointer ends too early"))?;
                    if !jumped {
                        self.position = position + 2;
                    }
                    jumped = true;
                    position = usize::from(u16::from_be_bytes([length & 0x3F, low]));
                }
                _ => return Err(malformed("a label type is unknown")),
            }
        }
        Err(malformed("a name has too many labels"))
    }
}

/// Reads an answer far enough to check its code and signature.
pub(crate) fn answer(message: &[u8]) -> Result<Answer> {
    if message.len() < HEADER {
        return Err(malformed("it is shorter than a header"));
    }
    let field = |index: usize| u16::from_be_bytes([message[index], message[index + 1]]);
    let mut reader = Reader {
        message,
        position: HEADER,
    };
    for _ in 0..field(4) {
        reader.name()?;
        reader.take(4)?;
    }
    let records = u32::from(field(6)) + u32::from(field(8)) + u32::from(field(10));
    let mut signature = None;
    for index in 0..records {
        let start = reader.position;
        let owner = reader.name()?;
        let kind = reader.u16()?;
        reader.take(6)?;
        let length = usize::from(reader.u16()?);
        let end = reader.position + length;
        if kind == TYPE_TSIG && index + 1 == records {
            let algorithm = reader.name()?;
            let time = reader.take(6)?;
            let time =
                u64::from_be_bytes([0, 0, time[0], time[1], time[2], time[3], time[4], time[5]]);
            let fudge = reader.u16()?;
            let size = usize::from(reader.u16()?);
            let mac = reader.take(size)?.to_vec();
            let original_id = reader.u16()?;
            let error = reader.u16()?;
            let other = usize::from(reader.u16()?);
            let other = reader.take(other)?.to_vec();
            signature = Some(Signature {
                start,
                key_name: owner,
                algorithm,
                time,
                fudge,
                mac,
                original_id,
                error,
                other,
            });
        }
        reader.position = end;
        if reader.position > message.len() {
            return Err(malformed("a record ends too early"));
        }
    }
    Ok(Answer {
        id: field(0),
        rcode: field(2) & 0x000F,
        signature,
    })
}

/// The signed part of a message: everything before its TSIG record, with
/// the record count it had and the ID it was signed with.
fn unsigned(message: &[u8], signature: &Signature) -> Vec<u8> {
    let mut unsigned = message[..signature.start].to_vec();
    let records = u16::from_be_bytes([unsigned[10], unsigned[11]]).saturating_sub(1);
    unsigned[10..12].copy_from_slice(&records.to_be_bytes());
    unsigned[0..2].copy_from_slice(&signature.original_id.to_be_bytes());
    unsigned
}

fn check_signature(signature: &Signature, key: &Key, now: u64) -> Result<()> {
    if signature.key_name != key.name || signature.algorithm != name(key.algorithm.as_str())? {
        return Err(PanelError::unavailable(
            "the DNS server signed its answer with another key",
        ));
    }
    if now.abs_diff(signature.time) > u64::from(signature.fudge) {
        return Err(PanelError::unavailable(
            "the DNS server's clock differs from this one by more than its signature allows",
        ));
    }
    Ok(())
}

/// Verifies the signature of an answer to a request signed with
/// `request_mac` (RFC 8945 §5.3.1).
pub(crate) fn verify_answer(
    message: &[u8],
    answer: &Answer,
    key: &Key,
    request_mac: &[u8],
    now: u64,
) -> Result<()> {
    let signature = answer
        .signature
        .as_ref()
        .ok_or_else(|| PanelError::unavailable("the DNS server did not sign its answer"))?;
    check_signature(signature, key, now)?;
    let size = u16::try_from(request_mac.len()).unwrap_or(0).to_be_bytes();
    let parts: [&[u8]; 4] = [
        &size,
        request_mac,
        &unsigned(message, signature),
        &variables(
            key,
            signature.time,
            signature.fudge,
            signature.error,
            &signature.other,
        )?,
    ];
    if key.algorithm.verify(&key.secret, &parts, &signature.mac) {
        Ok(())
    } else {
        Err(PanelError::unavailable(
            "the DNS server's answer has a signature that does not verify",
        ))
    }
}

/// Verifies the signature of a request, as a server does (RFC 8945 §5.2).
#[cfg(test)]
pub(crate) fn verify_request(message: &[u8], key: &Key, now: u64) -> Result<Vec<u8>> {
    let request = answer(message)?;
    let signature = request
        .signature
        .as_ref()
        .ok_or_else(|| PanelError::unavailable("the request is not signed"))?;
    check_signature(signature, key, now)?;
    let parts: [&[u8]; 2] = [
        &unsigned(message, signature),
        &variables(
            key,
            signature.time,
            signature.fudge,
            signature.error,
            &signature.other,
        )?,
    ];
    if key.algorithm.verify(&key.secret, &parts, &signature.mac) {
        Ok(signature.mac.clone())
    } else {
        Err(PanelError::unavailable(
            "the request's signature does not verify",
        ))
    }
}

/// Answers a request as a server does, signed over the request's MAC.
#[cfg(test)]
pub(crate) fn signed_answer(
    request: &[u8],
    rcode: u16,
    key: &Key,
    request_mac: &[u8],
    time: u64,
) -> Vec<u8> {
    let mut message = request[..HEADER].to_vec();
    message[2] |= 0x80;
    message[3] = (message[3] & 0xF0) | u8::try_from(rcode & 0x0F).unwrap_or(0);
    message[4..12].fill(0);
    let id = [message[0], message[1]];
    let size = u16::try_from(request_mac.len()).unwrap_or(0).to_be_bytes();
    let mac = key.algorithm.mac(
        &key.secret,
        &[
            &size,
            request_mac,
            &message,
            &variables(key, time, FUDGE, 0, &[]).unwrap(),
        ],
    );
    let mut data = name(key.algorithm.as_str()).unwrap();
    data.extend(&time.to_be_bytes()[2..]);
    data.extend(FUDGE.to_be_bytes());
    data.extend(u16::try_from(mac.len()).unwrap().to_be_bytes());
    data.extend(&mac);
    data.extend(id);
    data.extend([0, 0, 0, 0]);
    message.extend(&key.name);
    message.extend(TYPE_TSIG.to_be_bytes());
    message.extend(CLASS_ANY.to_be_bytes());
    message.extend(0_u32.to_be_bytes());
    message.extend(u16::try_from(data.len()).unwrap().to_be_bytes());
    message.extend(data);
    message[11] = 1;
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine};

    /// `nsupdate -v -y hmac-sha256:panel-test-key.:<secret>` from BIND 9.10
    /// adding `_acme-challenge.example.com. 60 TXT "LoqXcYV8…"`, signed at
    /// 1791566141 (2026-10-03T13:55:41Z).
    const NSUPDATE: &str = "0d9928000001000000010001076578616d706c6503636f6d00000600010f5f61636d652d6368616c6c656e6765c00c001000010000003c002c2b4c6f71586359563871354f4e624a5178626d52375343544e6f337469415844666f77796a78416a457558300e70616e656c2d746573742d6b65790000fa00ff00000000003d0b686d61632d7368613235360000006ac1093d012c00208414b93605e42a33063328d636e0a408390684bdd4751c55e51170d5300a91e10d9900000000";
    const SIGNED_AT: u64 = 0x6ac1_093d;

    fn key() -> Key {
        Key {
            name: name("panel-test-key.").unwrap(),
            algorithm: Algorithm::HmacSha256,
            secret: Zeroizing::new(
                STANDARD
                    .decode("c2VjcmV0IGZvciB0aGUgcGFuZWwgdGVzdCBrZXkgMjAyNg==")
                    .unwrap(),
            ),
        }
    }

    #[test]
    fn names_are_lowercase_labels() {
        assert_eq!(name("Example.COM.").unwrap(), b"\x07example\x03com\x00");
        assert_eq!(name(".").unwrap(), b"\x00");
        assert!(name("a..b").is_err());
        assert!(name(&"a".repeat(64)).is_err());
        assert!(name("bücher.example").is_err());
    }

    #[test]
    fn signatures_agree_with_bind() {
        let message = hex::decode(NSUPDATE).unwrap();
        verify_request(&message, &key(), SIGNED_AT + 10).expect("BIND's signature verifies");
        let mut tampered = message.clone();
        tampered[60] ^= 1;
        assert!(verify_request(&tampered, &key(), SIGNED_AT).is_err());
        assert!(
            verify_request(&message, &key(), SIGNED_AT + 301).is_err(),
            "too late"
        );
        let other = Key {
            secret: Zeroizing::new(b"another secret".to_vec()),
            ..key()
        };
        assert!(verify_request(&message, &other, SIGNED_AT).is_err());
    }

    #[test]
    fn updates_are_signed_as_bind_signs_them() {
        let zone = name("example.com").unwrap();
        let record = name("_acme-challenge.example.com").unwrap();
        let mut message = update(
            0x0d99,
            &zone,
            &record,
            "LoqXcYV8q5ONbJQxbmR7SCTNo3tiAXDfowyjxAjEuX0",
            60,
            Change::Add,
        )
        .unwrap();
        let mac = sign(&mut message, &key(), SIGNED_AT).unwrap();
        assert_eq!(verify_request(&message, &key(), SIGNED_AT).unwrap(), mac);
        // BIND compresses the record's name; everything else is the same.
        let bind = hex::decode(NSUPDATE).unwrap();
        assert_eq!(message[..12], bind[..12]);
        assert_eq!(
            &message[message.len() - 61..message.len() - 38],
            &bind[bind.len() - 61..bind.len() - 38]
        );

        let answer_bytes = signed_answer(&message, 0, &key(), &mac, SIGNED_AT + 1);
        let parsed = answer(&answer_bytes).unwrap();
        assert_eq!((parsed.id, parsed.rcode), (0x0d99, 0));
        verify_answer(&answer_bytes, &parsed, &key(), &mac, SIGNED_AT + 2).unwrap();
        assert!(verify_answer(&answer_bytes, &parsed, &key(), &[0; 32], SIGNED_AT).is_err());

        let removal = update(1, &zone, &record, "value", 60, Change::Remove).unwrap();
        let class_and_ttl = &removal[removal.len() - 14..removal.len() - 8];
        assert_eq!(class_and_ttl, [0, 254, 0, 0, 0, 0]);
    }

    #[test]
    fn malformed_answers_are_refused() {
        for bytes in [
            &[0_u8; 4][..],
            &[0, 1, 0x80, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0xC0][..],
        ] {
            assert!(answer(bytes).is_err(), "{bytes:?}");
        }
        let pointer_loop = [0, 1, 0x80, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0xC0, 12, 0, 6, 0, 1];
        assert!(answer(&pointer_loop).is_err());
    }
}
