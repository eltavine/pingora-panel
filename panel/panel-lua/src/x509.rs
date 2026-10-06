//! What the gateway reads of X.509 certificates (RFC 5280), in DER: their
//! fields as OCSP and nginx's `$ssl_client_*` variables name them.

use crate::tls::der;
use base64::Engine;

pub(crate) struct Element<'a> {
    pub(crate) tag: u8,
    pub(crate) content: &'a [u8],
    pub(crate) whole: &'a [u8],
}

pub(crate) fn element(input: &[u8]) -> Option<(Element<'_>, &[u8])> {
    let (tag, content, rest) = der(input)?;
    let whole = &input[..input.len() - rest.len()];
    Some((
        Element {
            tag,
            content,
            whole,
        },
        rest,
    ))
}

pub(crate) fn elements(mut content: &[u8]) -> Option<Vec<Element<'_>>> {
    let mut found = Vec::new();
    while !content.is_empty() {
        let (next, rest) = element(content)?;
        found.push(next);
        content = rest;
    }
    Some(found)
}

/// What a BIT STRING holds, when it uses every bit of its last byte.
pub(crate) fn bits<'a>(element: &Element<'a>) -> Option<&'a [u8]> {
    let (&unused, bits) = element.content.split_first()?;
    (element.tag == 0x03 && unused == 0).then_some(bits)
}

/// An element with `tag` around `content`.
pub(crate) fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut encoded = vec![tag];
    match content.len() {
        length @ 0..=0x7f => encoded.push(length as u8),
        length => {
            let bytes: Vec<u8> = length
                .to_be_bytes()
                .into_iter()
                .skip_while(|byte| *byte == 0)
                .collect();
            encoded.push(0x80 | bytes.len() as u8);
            encoded.extend(bytes);
        }
    }
    encoded.extend_from_slice(content);
    encoded
}

/// What the gateway reads of an X.509 certificate.
pub(crate) struct Certificate<'a> {
    pub(crate) tbs: &'a [u8],
    pub(crate) serial: &'a [u8],
    pub(crate) issuer: &'a [u8],
    pub(crate) subject: &'a [u8],
    pub(crate) key_algorithm: &'a [u8],
    pub(crate) key: &'a [u8],
    pub(crate) extensions: Option<&'a [u8]>,
    pub(crate) signature_algorithm: &'a [u8],
    pub(crate) signature: &'a [u8],
}

impl<'a> Certificate<'a> {
    pub(crate) fn parse(certificate: &'a [u8]) -> Option<Self> {
        let (outer, _) = element(certificate)?;
        let parts = elements(outer.content)?;
        let [tbs, algorithm, signature] = parts.as_slice() else {
            return None;
        };
        let mut fields = elements(tbs.content)?.into_iter().peekable();
        if fields.peek()?.tag == 0xa0 {
            fields.next();
        }
        let serial = fields.next()?;
        let _signature = fields.next()?;
        let issuer = fields.next()?;
        let _validity = fields.next()?;
        let subject = fields.next()?;
        let key_info = fields.next()?;
        let mut extensions = None;
        for field in fields {
            if field.tag == 0xa3 {
                extensions = Some(element(field.content)?.0.content);
            }
        }
        let key_parts = elements(key_info.content)?;
        let [key_algorithm, key] = key_parts.as_slice() else {
            return None;
        };
        Some(Self {
            tbs: tbs.whole,
            serial: serial.content,
            issuer: issuer.whole,
            subject: subject.whole,
            key_algorithm: key_algorithm.content,
            key: bits(key)?,
            extensions,
            signature_algorithm: algorithm.content,
            signature: bits(signature)?,
        })
    }

    /// The value of the extension `id`.
    pub(crate) fn extension(&self, id: &[u8]) -> Option<&'a [u8]> {
        for extension in elements(self.extensions?)? {
            let parts = elements(extension.content)?;
            let (name, rest) = parts.split_first()?;
            if name.tag == 0x06 && name.content == id {
                return rest
                    .last()
                    .filter(|value| value.tag == 0x04)
                    .map(|value| value.content);
            }
        }
        None
    }
}

impl Certificate<'_> {
    /// Its serial number in hex, as OpenSSL prints it.
    pub(crate) fn serial_hex(&self) -> String {
        let serial = match self.serial {
            [0, rest @ ..] if !rest.is_empty() => rest,
            serial => serial,
        };
        serial.iter().map(|byte| format!("{byte:02X}")).collect()
    }
}

/// The short names RFC 4514 and OpenSSL give attribute types.
const ATTRIBUTES: &[(&[u8], &str)] = &[
    (&[0x55, 0x04, 0x03], "CN"),
    (&[0x55, 0x04, 0x04], "SN"),
    (&[0x55, 0x04, 0x05], "serialNumber"),
    (&[0x55, 0x04, 0x06], "C"),
    (&[0x55, 0x04, 0x07], "L"),
    (&[0x55, 0x04, 0x08], "ST"),
    (&[0x55, 0x04, 0x09], "street"),
    (&[0x55, 0x04, 0x0a], "O"),
    (&[0x55, 0x04, 0x0b], "OU"),
    (&[0x55, 0x04, 0x0c], "title"),
    (&[0x55, 0x04, 0x2a], "GN"),
    (
        &[0x09, 0x92, 0x26, 0x89, 0x93, 0xf2, 0x2c, 0x64, 0x01, 0x19],
        "DC",
    ),
    (
        &[0x09, 0x92, 0x26, 0x89, 0x93, 0xf2, 0x2c, 0x64, 0x01, 0x01],
        "UID",
    ),
    (
        &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x01],
        "emailAddress",
    ),
];

/// An OID's dotted form.
fn dotted(oid: &[u8]) -> String {
    let mut parts = Vec::new();
    let mut value: u64 = 0;
    for (index, byte) in oid.iter().enumerate() {
        value = (value << 7) | u64::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            if parts.is_empty() {
                let first = (value / 40).min(2);
                parts.push(first);
                parts.push(value - first * 40);
            } else {
                parts.push(value);
            }
            value = 0;
        } else if index + 1 == oid.len() {
            return String::new();
        }
    }
    parts
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(".")
}

/// An attribute value as text: a string type decoded, anything else `#`
/// and its DER in hex (RFC 4514 §2.4).
fn attribute_value(value: &Element) -> String {
    let text = match value.tag {
        0x0c | 0x13 | 0x16 | 0x14 => String::from_utf8(value.content.to_vec()).ok(),
        0x1e => {
            let units: Vec<u16> = value
                .content
                .chunks(2)
                .map(|pair| u16::from_be_bytes([pair[0], *pair.get(1).unwrap_or(&0)]))
                .collect();
            String::from_utf16(&units).ok()
        }
        _ => None,
    };
    let Some(text) = text else {
        return format!(
            "#{}",
            value
                .whole
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        );
    };
    let last = text.chars().count().saturating_sub(1);
    let mut escaped = String::with_capacity(text.len());
    for (index, character) in text.chars().enumerate() {
        let special = matches!(character, ',' | '+' | '"' | '\\' | '<' | '>' | ';')
            || (index == 0 && matches!(character, '#' | ' '))
            || (index == last && character == ' ');
        if special {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

/// A Name as RFC 2253 writes it, as nginx's `$ssl_client_s_dn` reads:
/// its most specific part first, parts joined by commas.
pub(crate) fn distinguished_name(name: &[u8]) -> Option<String> {
    let (name, _) = element(name)?;
    let mut parts = Vec::new();
    for rdn in elements(name.content)? {
        let mut attributes = Vec::new();
        for attribute in elements(rdn.content)? {
            let pair = elements(attribute.content)?;
            let [kind, value] = pair.as_slice() else {
                return None;
            };
            let short = ATTRIBUTES
                .iter()
                .find(|(oid, _)| *oid == kind.content)
                .map_or_else(|| dotted(kind.content), |(_, name)| (*name).to_owned());
            attributes.push(format!("{short}={}", attribute_value(value)));
        }
        parts.push(attributes.join("+"));
    }
    parts.reverse();
    Some(parts.join(","))
}

/// A certificate as PEM, in lines of 64 characters.
pub(crate) fn pem(der: &[u8]) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(der);
    let mut text = String::from("-----BEGIN CERTIFICATE-----\n");
    for line in encoded.as_bytes().chunks(64) {
        text.push_str(std::str::from_utf8(line).unwrap_or_default());
        text.push('\n');
    }
    text.push_str("-----END CERTIFICATE-----\n");
    text
}
