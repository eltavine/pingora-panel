#![forbid(unsafe_code)]

//! A [`DnsProvider`] that publishes DNS-01 records through RFC 2136 dynamic
//! updates signed with TSIG (RFC 8945), which BIND, Knot DNS, PowerDNS and
//! most authoritative servers accept. Updates go to the zone's primary over
//! TCP, and its answers must carry a valid signature.

mod wire;

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine};
use panel_acme::DnsProvider;
use panel_errors::{PanelError, Result};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::timeout,
};
use wire::{Change, Key};
use zeroize::Zeroizing;

pub use wire::Algorithm;

const DEFAULT_TTL: u32 = 60;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

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
    key: Key,
    ttl: u32,
    timeout: Duration,
}

fn rcode_name(code: u16) -> &'static str {
    match code {
        1 => "FORMERR: the server could not read the update",
        2 => "SERVFAIL: the server failed to apply the update",
        3 => "NXDOMAIN",
        4 => "NOTIMP: the server does not accept updates",
        5 => "REFUSED: the server does not allow this update",
        6 => "YXDOMAIN",
        7 => "YXRRSET",
        8 => "NXRRSET",
        9 => "NOTAUTH: the server is not authoritative for the zone or does not accept the key",
        10 => "NOTZONE: the record is outside the zone",
        16 => "BADSIG: the server could not verify the signature",
        17 => "BADKEY: the server does not know the key",
        18 => "BADTIME: the clocks differ by too much",
        _ => "an unknown error",
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

fn zone_name(zone: &str) -> String {
    zone.trim().trim_end_matches('.').to_ascii_lowercase()
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
            wire::name(zone)?;
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
        Ok(Self {
            server: settings.server.trim().to_owned(),
            zones,
            key: Key {
                name: wire::name(&settings.key_name)?,
                algorithm: settings.algorithm,
                secret: Zeroizing::new(secret),
            },
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

    async fn send(&self, change: Change, record: &str, value: &str) -> Result<()> {
        let zone = self.zone_of(record)?;
        let mut id = [0_u8; 2];
        getrandom::fill(&mut id)
            .map_err(|error| PanelError::internal(format!("random generation failed: {error}")))?;
        let id = u16::from_be_bytes(id);
        let mut message = wire::update(
            id,
            &wire::name(zone)?,
            &wire::name(record)?,
            value,
            self.ttl,
            change,
        )?;
        let mac = wire::sign(&mut message, &self.key, now())?;
        let bytes = timeout(self.timeout, exchange(&self.server, &message))
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
        let answer = wire::answer(&bytes)?;
        if answer.id != id {
            return Err(PanelError::unavailable(
                "the DNS server answered another request",
            ));
        }
        let error = answer
            .signature
            .as_ref()
            .map(|signature| signature.error)
            .filter(|error| *error != 0);
        if answer.rcode != 0 || error.is_some() {
            let code = error.unwrap_or(answer.rcode);
            return Err(PanelError::validation_failed(format!(
                "the DNS server refused the update of {zone} ({})",
                rcode_name(code)
            )));
        }
        wire::verify_answer(&bytes, &answer, &self.key, &mac, now())
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
