#![forbid(unsafe_code)]

//! A [`DnsProvider`] that publishes DNS-01 records through RFC 2136 dynamic
//! updates signed with TSIG (RFC 8945), which BIND, Knot DNS, PowerDNS and
//! most authoritative servers accept. Messages and signatures come from
//! `hickory-proto` with `ring`; updates go to the zone's primary over TCP,
//! and its answers must carry a valid signature.

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine};
use hickory_proto::{
    op::{update_message, Message, ResponseCode},
    rr::{
        rdata::{
            tsig::{TsigAlgorithm, TsigError},
            TXT,
        },
        Name, RData, Record, RecordSet, TSigner,
    },
};
use panel_acme::DnsProvider;
use panel_errors::{PanelError, Result};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};
use zeroize::Zeroizing;

const DEFAULT_TTL: u32 = 60;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
/// Seconds of clock difference a signature tolerates (RFC 8945 §10).
const FUDGE: u16 = 300;

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

    fn tsig(self) -> TsigAlgorithm {
        match self {
            Self::HmacSha256 => TsigAlgorithm::HmacSha256,
            Self::HmacSha512 => TsigAlgorithm::HmacSha512,
        }
    }
}

/// Where and how to send updates.
#[derive(Clone)]
pub struct Rfc2136Settings {
    /// The primary server as `host:port`, usually port 53.
    pub server: String,
    /// The zones the key may update, such as `example.com`; a record goes to
    /// the longest zone that contains it.
    pub zones: Vec<String>,
    pub key_name: String,
    pub algorithm: Algorithm,
    /// The key's secret, base64-encoded as in BIND key files.
    pub secret: Zeroizing<String>,
    /// The TTL of published records; 60 seconds unless given.
    pub ttl: Option<u32>,
}

/// Publishes TXT records with RFC 2136 updates.
#[derive(Clone)]
pub struct Rfc2136 {
    server: String,
    /// Zone names, without the root's dot, longest first.
    zones: Vec<String>,
    signer: TSigner,
    ttl: u32,
    timeout: Duration,
}

/// Whether an update adds a record or removes it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Change {
    Add,
    Remove,
}

/// A fully qualified, lowercase domain name.
fn name(text: &str) -> Result<Name> {
    let invalid = || PanelError::invalid_argument(format!("{text:?} is not a DNS name"));
    if !text.is_ascii() {
        return Err(invalid());
    }
    let mut name = Name::from_ascii(text.trim()).map_err(|_| invalid())?;
    name.set_fqdn(true);
    Ok(name.to_lowercase())
}

fn zone_name(zone: &str) -> String {
    zone.trim().trim_end_matches('.').to_ascii_lowercase()
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// What a refusal means, with the TSIG error first since it explains the
/// response code it comes with (RFC 8945 §5.3).
fn refusal(code: ResponseCode, error: Option<&TsigError>) -> String {
    match (error, code) {
        (Some(TsigError::BadSig), _) => "BADSIG: the server could not verify the signature".into(),
        (Some(TsigError::BadKey), _) => "BADKEY: the server does not know the key".into(),
        (Some(TsigError::BadTime), _) => "BADTIME: the clocks differ by too much".into(),
        (Some(error), _) => format!("TSIG error {error:?}"),
        (None, ResponseCode::FormErr) => "FORMERR: the server could not read the update".into(),
        (None, ResponseCode::ServFail) => "SERVFAIL: the server failed to apply the update".into(),
        (None, ResponseCode::NotImp) => "NOTIMP: the server does not accept updates".into(),
        (None, ResponseCode::Refused) => "REFUSED: the server does not allow this update".into(),
        (None, ResponseCode::NotAuth) => {
            "NOTAUTH: the server is not authoritative for the zone or does not accept the key"
                .into()
        }
        (None, ResponseCode::NotZone) => "NOTZONE: the record is outside the zone".into(),
        (None, code) => code.to_str().into(),
    }
}

impl Rfc2136 {
    pub fn new(settings: Rfc2136Settings) -> Result<Self> {
        let secret = STANDARD
            .decode(settings.secret.trim())
            .ok()
            .filter(|secret| !secret.is_empty())
            .ok_or_else(|| PanelError::invalid_argument("the TSIG secret is not base64-encoded"))?;
        let mut zones = Vec::with_capacity(settings.zones.len());
        for zone in &settings.zones {
            name(zone)?;
            zones.push(zone_name(zone));
        }
        if zones.is_empty() || zones.iter().any(String::is_empty) {
            return Err(PanelError::invalid_argument(
                "name at least one zone the key may update",
            ));
        }
        zones.sort_by_key(|zone| std::cmp::Reverse(zone.len()));
        if settings.server.trim().is_empty() {
            return Err(PanelError::invalid_argument(
                "name the DNS server to update",
            ));
        }
        let signer = TSigner::new(
            secret,
            settings.algorithm.tsig(),
            name(&settings.key_name)?,
            FUDGE,
        )
        .map_err(|error| PanelError::invalid_argument(format!("the TSIG key: {error}")))?;
        Ok(Self {
            server: settings.server.trim().to_owned(),
            zones,
            signer,
            ttl: settings.ttl.unwrap_or(DEFAULT_TTL),
            timeout: DEFAULT_TIMEOUT,
        })
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The longest configured zone that contains `record`.
    fn zone_of(&self, record: &str) -> Result<&str> {
        let record = zone_name(record);
        self.zones
            .iter()
            .find(|zone| record == **zone || record.ends_with(&format!(".{zone}")))
            .map(String::as_str)
            .ok_or_else(|| {
                PanelError::validation_failed(format!(
                    "{record} is in none of the zones this DNS provider updates: {}",
                    self.zones.join(", ")
                ))
            })
    }

    /// An unsigned UPDATE of the record's zone that adds or removes the TXT
    /// record `value` at `record`; removing names the exact record (RFC 2136
    /// §2.5.1, §2.5.4).
    fn update(&self, change: Change, record: &str, value: &str) -> Result<Message> {
        let zone = name(self.zone_of(record)?)?;
        let record = name(record)?;
        if value.len() > 255 {
            return Err(PanelError::invalid_argument(
                "a TXT string holds at most 255 bytes",
            ));
        }
        let rrset = RecordSet::from(Record::from_rdata(
            record,
            self.ttl,
            RData::TXT(TXT::new(vec![value.to_owned()])),
        ));
        Ok(match change {
            Change::Add => update_message::append(rrset, zone, false, false),
            Change::Remove => update_message::delete_by_rdata(rrset, zone, false),
        })
    }

    async fn send(&self, change: Change, record: &str, value: &str) -> Result<()> {
        let zone = self.zone_of(record)?.to_owned();
        let mut message = self.update(change, record, value)?;
        let unsigned = || PanelError::internal("the update cannot be signed");
        let mut verifier = message
            .finalize(&self.signer, now())
            .map_err(|_| unsigned())?
            .ok_or_else(unsigned)?;
        let request = message
            .to_vec()
            .map_err(|error| PanelError::invalid_argument(format!("the update: {error}")))?;
        let bytes = timeout(self.timeout, exchange(&self.server, &request))
            .await
            .map_err(|_| {
                PanelError::deadline_exceeded(format!(
                    "the DNS server {} did not answer within {} seconds",
                    self.server,
                    self.timeout.as_secs()
                ))
                .retryable(true)
            })?
            .map_err(|error| {
                PanelError::unavailable(format!(
                    "cannot reach the DNS server {}: {error}",
                    self.server
                ))
                .retryable(true)
            })?;
        let answer = Message::from_vec(&bytes).map_err(|error| {
            PanelError::unavailable(format!("the DNS server sent a malformed answer: {error}"))
        })?;
        if answer.metadata.id != message.metadata.id {
            return Err(PanelError::unavailable(
                "the DNS server answered another request",
            ));
        }
        let error = answer
            .signature()
            .and_then(|signature| signature.data.error.as_ref());
        if answer.metadata.response_code != ResponseCode::NoError || error.is_some() {
            return Err(PanelError::validation_failed(format!(
                "the DNS server refused the update of {zone} ({})",
                refusal(answer.metadata.response_code, error)
            )));
        }
        verifier.verify(&bytes).map(drop).map_err(|error| {
            PanelError::unavailable(format!(
                "the DNS server's answer does not verify with the key: {error}"
            ))
        })
    }
}

/// Sends one message over TCP and reads its answer (RFC 1035 §4.2.2).
async fn exchange(server: &str, message: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut stream = TcpStream::connect(server).await?;
    let length = u16::try_from(message.len())
        .map_err(|_| std::io::Error::other("the update is too large"))?;
    let mut framed = Vec::with_capacity(message.len() + 2);
    framed.extend(length.to_be_bytes());
    framed.extend(message);
    stream.write_all(&framed).await?;
    let mut length = [0_u8; 2];
    stream.read_exact(&mut length).await?;
    let mut answer = vec![0; usize::from(u16::from_be_bytes(length))];
    stream.read_exact(&mut answer).await?;
    Ok(answer)
}

#[async_trait]
impl DnsProvider for Rfc2136 {
    async fn add_txt(&self, name: &str, value: &str) -> Result<()> {
        self.send(Change::Add, name, value).await
    }

    async fn remove_txt(&self, name: &str, value: &str) -> Result<()> {
        self.send(Change::Remove, name, value).await
    }
}

#[cfg(test)]
mod tests;
