//! The TLS handshakes `ssl_client_hello_by_lua` and `ssl_certificate_by_lua`
//! run in: what the client's hello says (RFC 8446 §4.1.2), and the
//! certificate and key the scripts choose instead of the gateway's.

use bytes::Bytes;

/// `server_name` (RFC 6066 §3).
const SERVER_NAME: u16 = 0;
/// `supported_versions` (RFC 8446 §4.2.1).
pub(crate) const SUPPORTED_VERSIONS: u16 = 43;
/// `pre_shared_key` (RFC 8446 §4.2.11).
const PRE_SHARED_KEY: u16 = 41;

/// A handshake as scripts see and change it.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct Handshake {
    /// The client's hello, as the handshake message it sent.
    pub client_hello: Bytes,
    pub server_name: Option<String>,
    /// The versions it offers, by their numbers on the wire (`0x0304` for
    /// TLS 1.3), in its order.
    pub versions: Vec<u16>,
    pub ciphers: Vec<u16>,
    /// Its extensions, by type, in its order.
    pub extensions: Vec<(u16, Bytes)>,
    pub random: Bytes,
    /// The session ID the hello offers to resume (`legacy_session_id`).
    pub session_id: Bytes,
    /// The identities of the tickets it offers to resume with.
    pub tickets: Vec<Bytes>,
    /// The session `ssl_session_*_by_lua` handlers run for, by its ID.
    pub session: Option<Bytes>,
    /// That session, serialized: what `ssl_session_store_by_lua` reads,
    /// and what `ssl_session_fetch_by_lua` found.
    pub serialized: Option<Bytes>,
    /// The cipher suite it settled on, by its OpenSSL name (`$ssl_cipher`).
    pub cipher: Option<String>,
    /// The version the handshake settles on, as far as the gateway knows:
    /// the newest the client offers, until the gateway says otherwise.
    pub version: Option<u16>,
    /// The DER certificate chain scripts set (`set_der_cert`, `set_cert`).
    pub chain: Option<Vec<Vec<u8>>>,
    /// The DER private key scripts set.
    pub key: Option<Vec<u8>>,
    /// `clear_certs`: the gateway's certificate is not presented.
    pub cleared: bool,
    /// The OCSP response stapled to the certificate presented
    /// (`ngx.ocsp.set_ocsp_status_resp`).
    pub ocsp: Option<Bytes>,
    /// The client certificate `ngx.ssl.verify_client` asks for.
    pub client_auth: Option<ClientAuth>,
    /// How the client's certificate verified, as `$ssl_client_verify` says
    /// on the connection's requests: `SUCCESS`, `FAILED:` and why, or
    /// `NONE`.
    pub client_verify: Option<String>,
    /// The certificates the client presented, its own first.
    pub client_chain: Vec<Vec<u8>>,
}

/// The client certificate `ngx.ssl.verify_client` asks for.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub struct ClientAuth {
    /// The DER certificates trusted to issue it.
    pub authorities: Vec<Vec<u8>>,
    /// The most intermediate certificates its chain may have, not counting
    /// those that issued themselves.
    pub depth: usize,
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        if self.0.len() < count {
            return None;
        }
        let (taken, rest) = self.0.split_at(count);
        self.0 = rest;
        Some(taken)
    }

    fn number(&mut self, size: usize) -> Option<usize> {
        Some(
            self.take(size)?
                .iter()
                .fold(0, |number, byte| (number << 8) | usize::from(*byte)),
        )
    }

    fn vector(&mut self, size: usize) -> Option<Reader<'a>> {
        let length = self.number(size)?;
        Some(Reader(self.take(length)?))
    }

    fn u16s(mut self) -> Vec<u16> {
        let mut found = Vec::new();
        while let Some(value) = self.number(2) {
            found.push(value as u16);
        }
        found
    }
}

impl Handshake {
    /// The hello `message` holds, a `ClientHello` handshake message.
    pub fn parse(message: &[u8]) -> Option<Self> {
        let mut outer = Reader(message);
        if outer.number(1)? != 1 {
            return None;
        }
        let mut body = outer.vector(3)?;
        let legacy = body.number(2)? as u16;
        let random = Bytes::copy_from_slice(body.take(32)?);
        let session_id = Bytes::copy_from_slice(body.vector(1)?.0);
        let ciphers = body.vector(2)?.u16s();
        body.vector(1)?;
        let mut extensions = Vec::new();
        if let Some(mut all) = body.vector(2) {
            while !all.0.is_empty() {
                let kind = all.number(2)? as u16;
                let data = all.vector(2)?;
                extensions.push((kind, Bytes::copy_from_slice(data.0)));
            }
        }
        let mut server_name = None;
        let mut versions = vec![legacy];
        let mut tickets = Vec::new();
        for (kind, data) in &extensions {
            let mut reader = Reader(data);
            match *kind {
                SERVER_NAME => {
                    let mut list = reader.vector(2)?;
                    while !list.0.is_empty() {
                        let kind = list.number(1)?;
                        let name = list.vector(2)?;
                        if kind == 0 {
                            server_name = std::str::from_utf8(name.0)
                                .ok()
                                .map(str::to_ascii_lowercase);
                            break;
                        }
                    }
                }
                SUPPORTED_VERSIONS => versions = reader.vector(1)?.u16s(),
                PRE_SHARED_KEY => {
                    let mut identities = reader.vector(2)?;
                    while !identities.0.is_empty() {
                        tickets.push(Bytes::copy_from_slice(identities.vector(2)?.0));
                        identities.take(4)?;
                    }
                }
                _ => {}
            }
        }
        let version = versions
            .iter()
            .copied()
            .filter(|version| !grease(*version))
            .max();
        Some(Self {
            client_hello: Bytes::copy_from_slice(message),
            server_name,
            versions,
            ciphers,
            extensions,
            random,
            session_id,
            tickets,
            version,
            ..Self::default()
        })
    }
}

/// Whether `value` is one of the values clients send to keep servers
/// tolerant of ones they do not know (GREASE, RFC 8701).
pub(crate) fn grease(value: u16) -> bool {
    value & 0x0f0f == 0x0a0a && value >> 8 == value & 0xff
}

/// How OpenSSL names a protocol version.
pub(crate) fn version_name(version: u16) -> Option<&'static str> {
    Some(match version {
        0x0300 => "SSLv3",
        0x0301 => "TLSv1",
        0x0302 => "TLSv1.1",
        0x0303 => "TLSv1.2",
        0x0304 => "TLSv1.3",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ClientHello for `shop.example` offering TLS 1.3 and 1.2.
    fn hello() -> Vec<u8> {
        let name = b"shop.example";
        let mut extensions = vec![0, 0];
        extensions.extend((name.len() as u16 + 5).to_be_bytes());
        extensions.extend((name.len() as u16 + 3).to_be_bytes());
        extensions.push(0);
        extensions.extend((name.len() as u16).to_be_bytes());
        extensions.extend(name);
        extensions.extend([0, 43, 0, 5, 4, 0x03, 0x04, 0x03, 0x03]);
        let mut body = vec![0x03, 0x03];
        body.extend([7u8; 32]);
        body.push(0);
        body.extend([0, 4, 0x13, 0x01, 0xc0, 0x2f]);
        body.extend([1, 0]);
        body.extend((extensions.len() as u16).to_be_bytes());
        body.extend(extensions);
        let mut message = vec![1];
        message.extend(&(body.len() as u32).to_be_bytes()[1..]);
        message.extend(body);
        message
    }

    #[test]
    fn hellos_give_their_name_versions_ciphers_and_extensions() {
        let handshake = Handshake::parse(&hello()).unwrap();
        assert_eq!(handshake.server_name.as_deref(), Some("shop.example"));
        assert_eq!(handshake.versions, [0x0304, 0x0303]);
        assert_eq!(handshake.version, Some(0x0304));
        assert_eq!(handshake.ciphers, [0x1301, 0xc02f]);
        let kinds: Vec<u16> = handshake.extensions.iter().map(|(kind, _)| *kind).collect();
        assert_eq!(kinds, [SERVER_NAME, SUPPORTED_VERSIONS]);
        assert_eq!(handshake.random.len(), 32);
        assert!(Handshake::parse(b"\x02\x00\x00\x00").is_none());
        assert!(Handshake::parse(&hello()[..20]).is_none());
        assert_eq!(version_name(0x0304), Some("TLSv1.3"));
        assert!(grease(0x1a1a) && grease(0xfafa) && !grease(0x0a1a) && !grease(0x0304));
        assert!(handshake.session_id.is_empty() && handshake.tickets.is_empty());
    }

    #[test]
    fn hellos_give_the_sessions_they_offer_to_resume() {
        let identity = b"ticket-1";
        let mut psk = vec![0, 41];
        let identities_len = 2 + identity.len() + 4;
        let binders = [0u8, 33, 32];
        let ext_len = 2 + identities_len + binders.len() + 32;
        psk.extend((ext_len as u16).to_be_bytes());
        psk.extend((identities_len as u16).to_be_bytes());
        psk.extend((identity.len() as u16).to_be_bytes());
        psk.extend(identity);
        psk.extend([0, 0, 0, 1]);
        psk.extend(binders);
        psk.extend([9u8; 32]);
        let mut body = vec![0x03, 0x03];
        body.extend([7u8; 32]);
        body.push(32);
        body.extend([5u8; 32]);
        body.extend([0, 2, 0x13, 0x01, 1, 0]);
        body.extend((psk.len() as u16).to_be_bytes());
        body.extend(psk);
        let mut message = vec![1];
        message.extend(&(body.len() as u32).to_be_bytes()[1..]);
        message.extend(body);
        let handshake = Handshake::parse(&message).unwrap();
        assert_eq!(&handshake.session_id[..], &[5u8; 32]);
        assert_eq!(handshake.tickets, [Bytes::from_static(b"ticket-1")]);
    }
}
