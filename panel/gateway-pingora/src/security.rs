//! Request security policies (ADR 0017), compiled per snapshot: client
//! networks, methods, paths, user agents, referring pages, Basic
//! authentication, size and time limits, rate limits and concurrency caps.

use crate::secrets::SecretSource;
use base64::{engine::general_purpose::STANDARD, Engine};
use governor::{
    clock::{Clock, DefaultClock},
    middleware::NoOpMiddleware,
    state::keyed::HashMapStateStore,
    Quota, RateLimiter,
};
use http::{header, HeaderName, Method};
use panel_domain::IpNetwork;
use panel_errors::{PanelError, Result};
use panel_ir::{RateLimitKey, RealIpHeader, SecurityPolicy};
use parking_lot::Mutex;
use pingora_http::RequestHeader;
use regex::{RegexSet, RegexSetBuilder};
use sha2::{Digest, Sha256};
use std::{
    collections::{hash_map::DefaultHasher, BTreeSet, HashMap},
    hash::{Hash, Hasher},
    net::IpAddr,
    num::NonZeroU32,
    sync::{Arc, LazyLock, Weak},
    time::{Duration, Instant},
};

/// How long a verified password is remembered.
const VERIFIED_FOR: Duration = Duration::from_secs(300);
const MAX_VERIFIED: usize = 4096;
const SHARDS: usize = 64;
/// Keys per rate limit shard before idle ones are dropped.
const MAX_KEYS_PER_SHARD: usize = 4096;

/// Salts the digests of remembered credentials for this process.
static CREDENTIAL_SALT: LazyLock<[u8; 32]> = LazyLock::new(|| {
    let mut salt = [0_u8; 32];
    let _ = getrandom::fill(&mut salt);
    salt
});

/// Client networks in CIDR notation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Networks(Vec<IpNetwork>);

impl Networks {
    pub(crate) fn parse<'a>(cidrs: impl IntoIterator<Item = &'a String>) -> Result<Self> {
        cidrs
            .into_iter()
            .map(|cidr| {
                IpNetwork::new(cidr.trim())
                    .map_err(|error| PanelError::validation_failed(error.to_string()))
            })
            .collect::<Result<_>>()
            .map(Self)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(crate) fn contains(&self, address: IpAddr) -> bool {
        let address = canonical(address);
        self.0.iter().any(|network| network.contains(address))
    }
}

/// IPv4-mapped IPv6 addresses as the IPv4 addresses they carry.
pub(crate) fn canonical(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(address, IpAddr::V4),
        IpAddr::V4(_) => address,
    }
}

/// How a listener learns the client's address from trusted proxies.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ClientResolution {
    pub trusted: Networks,
    pub header: RealIpHeader,
}

impl ClientResolution {
    /// The client: the peer, or, walking back from a trusted peer, the
    /// first address not in a trusted network. Also whether the peer is
    /// trusted to forward.
    pub(crate) fn resolve(&self, peer: IpAddr, request: &RequestHeader) -> (IpAddr, bool) {
        let peer = canonical(peer);
        if self.trusted.is_empty() || !self.trusted.contains(peer) {
            return (peer, false);
        }
        let named: Vec<IpAddr> = match self.header {
            RealIpHeader::XRealIp => header_values(request, "x-real-ip")
                .filter_map(|value| parse_address(value.trim()))
                .collect(),
            RealIpHeader::Forwarded => header_values(request, "forwarded")
                .flat_map(|value| value.split(','))
                .filter_map(|element| {
                    element
                        .split(';')
                        .find_map(|pair| {
                            let (name, value) = pair.split_once('=')?;
                            name.trim().eq_ignore_ascii_case("for").then_some(value)
                        })
                        .and_then(|value| parse_address(value.trim().trim_matches('"')))
                })
                .collect(),
            _ => header_values(request, "x-forwarded-for")
                .flat_map(|value| value.split(','))
                .filter_map(|value| parse_address(value.trim()))
                .collect(),
        };
        let client = named
            .iter()
            .rev()
            .map(|address| canonical(*address))
            .find(|address| !self.trusted.contains(*address))
            .or_else(|| named.first().copied().map(canonical))
            .unwrap_or(peer);
        (client, true)
    }
}

fn header_values<'a>(
    request: &'a RequestHeader,
    name: &'static str,
) -> impl Iterator<Item = &'a str> {
    request
        .headers
        .get_all(name)
        .into_iter()
        .filter_map(|value| value.to_str().ok())
}

/// An address as forwarding headers write it: bare, `[v6]`, or with a port.
fn parse_address(value: &str) -> Option<IpAddr> {
    if let Ok(address) = value.parse() {
        return Some(address);
    }
    if let Some(rest) = value.strip_prefix('[') {
        return rest.split_once(']')?.0.parse().ok();
    }
    value.rsplit_once(':')?.0.parse().ok()
}

/// Why a request was refused, and the answer it gets.
#[derive(Debug)]
pub(crate) struct Refusal {
    pub status: u16,
    pub message: String,
    pub headers: Vec<(HeaderName, String)>,
    /// A body and content type from the policy, sent as they are.
    pub custom: Option<(String, Option<String>)>,
}

impl Refusal {
    fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            headers: Vec::new(),
            custom: None,
        }
    }
}

/// A request as the checks see it.
pub(crate) struct Candidate<'a> {
    pub client: IpAddr,
    pub header: &'a RequestHeader,
    pub path: &'a str,
    pub host: &'a str,
    pub route: &'a str,
}

/// Held while a request is in progress, for concurrency caps.
pub(crate) struct Permit {
    gate: Arc<Concurrency>,
    key: IpAddr,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut counts = self.gate.counts.lock();
        if let Some(count) = counts.get_mut(&self.key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                counts.remove(&self.key);
            }
        }
    }
}

/// What an admitted request still has to keep to.
#[derive(Default)]
pub(crate) struct Admission {
    pub permits: Vec<Permit>,
    pub max_body_bytes: Option<u64>,
    pub body_timeout: Option<Duration>,
    pub strip_authorization: bool,
}

struct Concurrency {
    limit: u64,
    counts: Mutex<HashMap<IpAddr, u64>>,
}

type KeyedLimiter<C> =
    RateLimiter<String, HashMapStateStore<String>, C, NoOpMiddleware<<C as Clock>::Instant>>;

/// One rate limit's GCRA state (the leaky bucket of nginx `limit_req`),
/// sharded by key.
struct Buckets<C: Clock = DefaultClock> {
    shards: Box<[KeyedLimiter<C>]>,
    clock: C,
}

impl Buckets {
    fn new(requests: u64, per_seconds: u64, burst: u64) -> Self {
        Self::with_clock(requests, per_seconds, burst, DefaultClock::default())
    }
}

impl<C: Clock + Clone> Buckets<C> {
    fn with_clock(requests: u64, per_seconds: u64, burst: u64, clock: C) -> Self {
        let period = u128::from(per_seconds) * 1_000_000_000 / u128::from(requests.max(1));
        let burst = u32::try_from(burst.saturating_add(1))
            .ok()
            .and_then(NonZeroU32::new)
            .unwrap_or(NonZeroU32::MAX);
        let quota = Quota::with_period(Duration::from_nanos(
            u64::try_from(period).unwrap_or(u64::MAX),
        ))
        .unwrap_or_else(|| Quota::per_second(NonZeroU32::MAX))
        .allow_burst(burst);
        Self {
            shards: (0..SHARDS)
                .map(|_| RateLimiter::hashmap_with_clock(quota, clock.clone()))
                .collect(),
            clock,
        }
    }

    /// Admits a request for `key`, or tells how long until one would be.
    fn take(&self, key: String) -> std::result::Result<(), Duration> {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        let shard = &self.shards[(hasher.finish() as usize) % SHARDS];
        if shard.len() >= MAX_KEYS_PER_SHARD {
            shard.retain_recent();
        }
        shard
            .check_key(&key)
            .map_err(|not_until| not_until.wait_time_from(self.clock.now()))
    }
}

/// Rate limit and concurrency state kept across snapshots while a policy's
/// limit stays the same.
#[derive(Default)]
pub(crate) struct LimitState {
    buckets: Mutex<HashMap<String, Weak<Buckets>>>,
    concurrency: Mutex<HashMap<String, Weak<Concurrency>>>,
}

impl LimitState {
    fn buckets(&self, key: String, make: impl FnOnce() -> Buckets) -> Arc<Buckets> {
        let mut known = self.buckets.lock();
        known.retain(|_, weak| weak.strong_count() > 0);
        if let Some(existing) = known.get(&key).and_then(Weak::upgrade) {
            return existing;
        }
        let created = Arc::new(make());
        known.insert(key, Arc::downgrade(&created));
        created
    }

    fn concurrency(&self, key: String, limit: u64) -> Arc<Concurrency> {
        let mut known = self.concurrency.lock();
        known.retain(|_, weak| weak.strong_count() > 0);
        if let Some(existing) = known.get(&key).and_then(Weak::upgrade) {
            return existing;
        }
        let created = Arc::new(Concurrency {
            limit,
            counts: Mutex::new(HashMap::new()),
        });
        known.insert(key, Arc::downgrade(&created));
        created
    }
}

struct Limiter {
    key: RateLimitKey,
    buckets: Arc<Buckets>,
}

enum StoredHash {
    Bcrypt(String),
    Argon2(String),
}

struct BasicAuthGate {
    challenge: String,
    users: HashMap<String, Arc<StoredHash>>,
    verified: Mutex<HashMap<[u8; 32], Instant>>,
}

impl BasicAuthGate {
    fn parse(realm: &str, file: &str, id: &str) -> Result<Self> {
        let mut users = HashMap::new();
        for (number, line) in file.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let invalid = |why: &str| {
                PanelError::validation_failed(format!(
                    "line {} of the password file {id} {why}",
                    number + 1
                ))
            };
            let (user, hash) = line
                .split_once(':')
                .ok_or_else(|| invalid("is not user:hash"))?;
            let hash = if ["$2a$", "$2b$", "$2y$"]
                .iter()
                .any(|prefix| hash.starts_with(prefix))
            {
                StoredHash::Bcrypt(hash.to_owned())
            } else if hash.starts_with("$argon2id$") || hash.starts_with("$argon2i$") {
                StoredHash::Argon2(hash.to_owned())
            } else {
                return Err(invalid("uses a weak or unknown hash; use bcrypt or Argon2"));
            };
            users.insert(user.to_owned(), Arc::new(hash));
        }
        let realm = realm.replace(['"', '\\'], "");
        Ok(Self {
            challenge: format!("Basic realm=\"{realm}\", charset=\"UTF-8\""),
            users,
            verified: Mutex::new(HashMap::new()),
        })
    }

    fn refusal(&self) -> Refusal {
        let mut refusal = Refusal::new(401, "authentication is required");
        refusal
            .headers
            .push((header::WWW_AUTHENTICATE, self.challenge.clone()));
        refusal
    }

    async fn check(&self, request: &RequestHeader) -> std::result::Result<(), Refusal> {
        let Some((user, password)) = request
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| {
                let (scheme, encoded) = value.split_once(' ')?;
                scheme
                    .eq_ignore_ascii_case("basic")
                    .then_some(encoded.trim())
            })
            .and_then(|encoded| STANDARD.decode(encoded).ok())
            .and_then(|decoded| String::from_utf8(decoded).ok())
            .and_then(|credentials| {
                let (user, password) = credentials.split_once(':')?;
                Some((user.to_owned(), password.to_owned()))
            })
        else {
            return Err(self.refusal());
        };
        let Some(hash) = self.users.get(&user).cloned() else {
            return Err(self.refusal());
        };
        let digest: [u8; 32] = Sha256::new()
            .chain_update(*CREDENTIAL_SALT)
            .chain_update(user.as_bytes())
            .chain_update([0])
            .chain_update(password.as_bytes())
            .finalize()
            .into();
        let now = Instant::now();
        if self
            .verified
            .lock()
            .get(&digest)
            .is_some_and(|at| now.duration_since(*at) < VERIFIED_FOR)
        {
            return Ok(());
        }
        let matches = tokio::task::spawn_blocking(move || match hash.as_ref() {
            StoredHash::Bcrypt(hash) => bcrypt::verify(&password, hash).unwrap_or(false),
            StoredHash::Argon2(hash) => {
                use argon2::{
                    password_hash::{phc::PasswordHash, PasswordVerifier},
                    Argon2,
                };
                PasswordHash::new(hash).is_ok_and(|parsed| {
                    Argon2::default()
                        .verify_password(password.as_bytes(), &parsed)
                        .is_ok()
                })
            }
        })
        .await
        .unwrap_or(false);
        if !matches {
            return Err(self.refusal());
        }
        let mut verified = self.verified.lock();
        if verified.len() >= MAX_VERIFIED {
            verified.retain(|_, at| now.duration_since(*at) < VERIFIED_FOR);
            if verified.len() >= MAX_VERIFIED {
                verified.clear();
            }
        }
        verified.insert(digest, now);
        Ok(())
    }
}

/// A referring host pattern: a host, or `*.` and a parent for any of its
/// subdomains.
struct Referers {
    hosts: Vec<String>,
    allow_empty: bool,
}

impl Referers {
    fn allows(&self, request: &RequestHeader) -> bool {
        let Some(referer) = request
            .headers
            .get(header::REFERER)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return self.allow_empty;
        };
        let Some(host) = referer_host(referer) else {
            return false;
        };
        self.hosts
            .iter()
            .any(|pattern| match pattern.strip_prefix("*.") {
                Some(parent) => host
                    .strip_suffix(parent)
                    .is_some_and(|rest| rest.ends_with('.') && rest.len() > 1),
                None => host == *pattern,
            })
    }
}

/// The host of an absolute URL, lowercased, without user or port.
fn referer_host(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = if let Some(bracketed) = host.strip_prefix('[') {
        bracketed.split_once(']')?.0
    } else {
        host.split_once(':').map_or(host, |(host, _)| host)
    };
    (!host.is_empty()).then(|| host.trim_end_matches('.').to_ascii_lowercase())
}

/// A compiled security policy.
pub(crate) struct SecurityGate {
    allowed: Networks,
    denied: Networks,
    methods: Option<(BTreeSet<String>, String)>,
    denied_paths: Vec<String>,
    agents: Option<RegexSet>,
    referers: Option<Referers>,
    basic: Option<BasicAuthGate>,
    max_header_bytes: Option<usize>,
    max_body_bytes: Option<u64>,
    body_timeout: Option<Duration>,
    limiters: Vec<Limiter>,
    concurrency: Option<Arc<Concurrency>>,
    limited: Option<(u16, String, Option<String>)>,
}

impl SecurityGate {
    pub(crate) fn compile(
        policy: &SecurityPolicy,
        secrets: &dyn SecretSource,
        state: &LimitState,
    ) -> Result<Self> {
        let id = &policy.id;
        let agents = (!policy.denied_user_agents.is_empty())
            .then(|| {
                RegexSetBuilder::new(&policy.denied_user_agents)
                    .case_insensitive(true)
                    .size_limit(1 << 20)
                    .build()
                    .map_err(|error| {
                        PanelError::validation_failed(format!(
                            "security policy {id} has an invalid user agent pattern: {error}"
                        ))
                    })
            })
            .transpose()?;
        let basic = policy
            .basic_auth
            .as_ref()
            .map(|auth| {
                let file = secrets.read(&auth.users_secret_id)?;
                let file = String::from_utf8(file).map_err(|_| {
                    PanelError::validation_failed(format!(
                        "the password file {} is not UTF-8",
                        auth.users_secret_id
                    ))
                })?;
                BasicAuthGate::parse(&auth.realm, &file, &auth.users_secret_id)
            })
            .transpose()?;
        let mut rules = policy.rate_limits.clone();
        if let Some(rate) = policy.request_rate_per_second {
            rules.push(panel_ir::RateLimit {
                key: RateLimitKey::ClientAddress,
                requests: rate,
                per_seconds: 1,
                burst: rate,
            });
        }
        let limiters = rules
            .into_iter()
            .enumerate()
            .map(|(index, rule)| {
                let fingerprint = format!(
                    "{id}/{index}/{:?}/{}/{}/{}",
                    rule.key, rule.requests, rule.per_seconds, rule.burst
                );
                Limiter {
                    buckets: state.buckets(fingerprint, || {
                        Buckets::new(rule.requests, rule.per_seconds, rule.burst)
                    }),
                    key: rule.key,
                }
            })
            .collect();
        let methods = (!policy.allowed_methods.is_empty()).then(|| {
            let methods: BTreeSet<String> = policy
                .allowed_methods
                .iter()
                .map(|method| method.to_ascii_uppercase())
                .collect();
            let mut listed = methods.clone();
            if listed.contains("GET") {
                listed.insert("HEAD".to_owned());
            }
            let allow = listed.into_iter().collect::<Vec<_>>().join(", ");
            (methods, allow)
        });
        Ok(Self {
            allowed: Networks::parse(&policy.allowed_cidrs)?,
            denied: Networks::parse(&policy.denied_cidrs)?,
            methods,
            denied_paths: policy.denied_path_prefixes.clone(),
            agents,
            referers: policy.referer.as_ref().map(|rule| Referers {
                hosts: rule
                    .allowed_hosts
                    .iter()
                    .map(|host| host.trim_end_matches('.').to_ascii_lowercase())
                    .collect(),
                allow_empty: rule.allow_empty,
            }),
            basic,
            max_header_bytes: policy
                .max_header_bytes
                .map(|bytes| usize::try_from(bytes).unwrap_or(usize::MAX)),
            max_body_bytes: policy.max_body_bytes,
            body_timeout: policy.body_timeout_ms.map(Duration::from_millis),
            limiters,
            concurrency: policy
                .max_concurrent_requests
                .map(|limit| state.concurrency(format!("{id}/{limit}"), limit)),
            limited: policy.limited_response.as_ref().map(|response| {
                (
                    response.status,
                    response.body.clone(),
                    response.content_type.clone(),
                )
            }),
        })
    }

    fn limited(&self, message: String, retry_after: Option<Duration>) -> Refusal {
        let mut refusal = Refusal::new(429, message);
        if let Some((status, body, content_type)) = &self.limited {
            refusal.status = *status;
            refusal.custom = Some((body.clone(), content_type.clone()));
        }
        if let Some(wait) = retry_after {
            let seconds = wait.as_secs() + u64::from(wait.subsec_nanos() > 0);
            refusal
                .headers
                .push((header::RETRY_AFTER, seconds.max(1).to_string()));
        }
        refusal
    }

    /// Admits a request or tells how to refuse it, adding what the request
    /// must still keep to and the permits it holds to `admission`.
    pub(crate) async fn check(
        &self,
        request: &Candidate<'_>,
        admission: &mut Admission,
    ) -> std::result::Result<(), Refusal> {
        let header = request.header;
        if self.denied.contains(request.client)
            || (!self.allowed.is_empty() && !self.allowed.contains(request.client))
        {
            return Err(Refusal::new(403, "this address may not use this site"));
        }
        if let Some((methods, allow)) = &self.methods {
            if !methods.contains(header.method.as_str())
                && !(header.method == Method::HEAD && methods.contains("GET"))
            {
                let mut refusal = Refusal::new(405, "this method is not allowed here");
                refusal.headers.push((header::ALLOW, allow.clone()));
                return Err(refusal);
            }
        }
        if self
            .denied_paths
            .iter()
            .any(|prefix| request.path.starts_with(prefix.as_str()))
        {
            return Err(Refusal::new(403, "this path is not available"));
        }
        if let Some(agents) = &self.agents {
            let agent = header
                .headers
                .get(header::USER_AGENT)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("");
            if agents.is_match(agent) {
                return Err(Refusal::new(403, "this client may not use this site"));
            }
        }
        if let Some(referers) = &self.referers {
            if !referers.allows(header) {
                return Err(Refusal::new(
                    403,
                    "this resource may not be linked from there",
                ));
            }
        }
        if let Some(limit) = self.max_header_bytes {
            let bytes: usize = header
                .headers
                .iter()
                .map(|(name, value)| name.as_str().len() + value.len() + 4)
                .sum();
            if bytes > limit {
                return Err(Refusal::new(431, "the request headers are too large"));
            }
        }
        if let Some(limit) = self.max_body_bytes {
            let declared = header
                .headers
                .get(header::CONTENT_LENGTH)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.trim().parse::<u64>().ok());
            if declared.is_some_and(|length| length > limit) {
                return Err(Refusal::new(413, "the request body is too large"));
            }
            admission.max_body_bytes = Some(
                admission
                    .max_body_bytes
                    .map_or(limit, |current| current.min(limit)),
            );
        }
        for limiter in &self.limiters {
            let key = match &limiter.key {
                RateLimitKey::ClientAddress => request.client.to_string(),
                RateLimitKey::Host => request.host.to_owned(),
                RateLimitKey::Route => request.route.to_owned(),
                RateLimitKey::Header { name } => header
                    .headers
                    .get(name.as_str())
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or("")
                    .to_owned(),
                _ => String::new(),
            };
            if let Err(wait) = limiter.buckets.take(key) {
                return Err(self.limited("too many requests".into(), Some(wait)));
            }
        }
        if let Some(gate) = &self.concurrency {
            let mut counts = gate.counts.lock();
            let count = counts.entry(request.client).or_insert(0);
            if *count >= gate.limit {
                drop(counts);
                return Err(self.limited("too many requests in progress".into(), None));
            }
            *count += 1;
            drop(counts);
            admission.permits.push(Permit {
                gate: Arc::clone(gate),
                key: request.client,
            });
        }
        if let Some(basic) = &self.basic {
            basic.check(header).await?;
            admission.strip_authorization = true;
        }
        if let Some(timeout) = self.body_timeout {
            admission.body_timeout = Some(
                admission
                    .body_timeout
                    .map_or(timeout, |current| current.min(timeout)),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use governor::clock::FakeRelativeClock;

    fn networks(cidrs: &[&str]) -> Networks {
        let cidrs: Vec<String> = cidrs.iter().map(|cidr| (*cidr).to_owned()).collect();
        Networks::parse(&cidrs).unwrap()
    }

    #[test]
    fn networks_match_by_prefix_and_ipv4_mapped_addresses_as_ipv4() {
        let set = networks(&["10.0.0.0/8", "192.0.2.1", "2001:db8::/32"]);
        for inside in ["10.1.2.3", "192.0.2.1", "::ffff:10.0.0.1", "2001:db8::1"] {
            assert!(set.contains(inside.parse().unwrap()), "{inside}");
        }
        for outside in ["11.0.0.1", "192.0.2.2", "2001:db9::1"] {
            assert!(!set.contains(outside.parse().unwrap()), "{outside}");
        }
        assert!(networks(&["0.0.0.0/0"]).contains("203.0.113.9".parse().unwrap()));
        for invalid in ["10.0.0.0/33", "nope", "2001:db8::/129"] {
            assert!(Networks::parse(&[invalid.to_owned()]).is_err(), "{invalid}");
        }
    }

    fn request(headers: &[(&str, &str)]) -> RequestHeader {
        let mut request = RequestHeader::build("GET", b"/", None).unwrap();
        for (name, value) in headers {
            request.append_header(name.to_string(), *value).unwrap();
        }
        request
    }

    #[test]
    fn clients_are_the_first_untrusted_address_from_the_peer_back() {
        let resolution = ClientResolution {
            trusted: networks(&["10.0.0.0/8"]),
            header: RealIpHeader::XForwardedFor,
        };
        let forwarded = request(&[("x-forwarded-for", "198.51.100.7, 203.0.113.9, 10.0.0.2")]);
        assert_eq!(
            resolution.resolve("10.0.0.1".parse().unwrap(), &forwarded),
            ("203.0.113.9".parse().unwrap(), true)
        );
        assert_eq!(
            resolution.resolve("192.0.2.5".parse().unwrap(), &forwarded),
            ("192.0.2.5".parse().unwrap(), false),
            "untrusted peers are the client whatever they claim"
        );
        let rfc7239 = ClientResolution {
            header: RealIpHeader::Forwarded,
            ..resolution.clone()
        };
        let elements = request(&[(
            "forwarded",
            "for=192.0.2.60;proto=https, for=\"[2001:db8::7]:4711\"",
        )]);
        assert_eq!(
            rfc7239.resolve("10.0.0.1".parse().unwrap(), &elements).0,
            "2001:db8::7".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn buckets_admit_the_burst_then_the_rate() {
        let clock = FakeRelativeClock::default();
        let buckets = Buckets::with_clock(2, 1, 1, clock.clone());
        assert!(buckets.take("a".into()).is_ok());
        assert!(buckets.take("a".into()).is_ok());
        let wait = buckets.take("a".into()).unwrap_err();
        assert!(wait <= Duration::from_millis(500) && wait > Duration::ZERO);
        assert!(buckets.take("b".into()).is_ok(), "keys are separate");
        clock.advance(Duration::from_millis(500));
        assert!(buckets.take("a".into()).is_ok());
        assert!(buckets.take("a".into()).is_err());
    }

    #[test]
    fn referers_match_hosts_and_subdomains() {
        let referers = Referers {
            hosts: vec!["example.com".into(), "*.example.org".into()],
            allow_empty: false,
        };
        for allowed in [
            "https://example.com/page",
            "http://cdn.example.org:8080/x",
            "https://a.b.example.org",
        ] {
            assert!(
                referers.allows(&request(&[("referer", allowed)])),
                "{allowed}"
            );
        }
        for refused in [
            "https://evil.com/",
            "https://example.org/",
            "https://notexample.com",
        ] {
            assert!(
                !referers.allows(&request(&[("referer", refused)])),
                "{refused}"
            );
        }
        assert!(!referers.allows(&request(&[])));
        assert_eq!(
            referer_host("https://user@Example.COM:443/"),
            Some("example.com".into())
        );
    }

    #[test]
    fn password_files_take_only_strong_hashes() {
        let bcrypt = bcrypt::hash("secret", 4).unwrap();
        let gate = BasicAuthGate::parse(
            "Staff \"area\"",
            &format!("# staff\nalice:{bcrypt}\n"),
            "staff",
        )
        .unwrap();
        assert_eq!(
            gate.challenge,
            "Basic realm=\"Staff area\", charset=\"UTF-8\""
        );
        assert!(gate.users.contains_key("alice"));
        for weak in ["bob:$apr1$abc$def", "bob:{SHA}abc", "bob:plain"] {
            assert!(BasicAuthGate::parse("x", weak, "staff").is_err(), "{weak}");
        }
    }
}
