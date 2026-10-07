//! The proxy cache (ADR 0043): plans compiled from cache policies, the
//! gateway's store with Pingora's cache lock and predictor, purges by
//! generation and by key, and counts of what the cache did.

mod store;

use crate::template::{Facts, Template};
use http::{header, HeaderName};
use panel_errors::{PanelError, Result};
use panel_ir::CachePolicy;
use panel_routing::{CompiledConditions, Request};
use parking_lot::Mutex;
use pingora_cache::{
    cache_control::{CacheControl, Cacheable, InterpretCacheControl},
    filters::calculate_expires_header_time,
    key::{CacheHashKey, HashBinary},
    lock::{CacheKeyLockImpl, CacheLock},
    predictor::Predictor,
    CacheKey, CacheMeta, CacheOptionOverrides, CachePhase, NoCacheReason, RespCacheable,
    VarianceBuilder,
};
use pingora_http::{RequestHeader, ResponseHeader};
use pingora_proxy::Session;
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime},
};
pub(crate) use store::{CacheStore, StoreUsage};

/// How long a miss holds its key before waiting requests stop waiting on it.
const LOCK_AGE: Duration = Duration::from_secs(10);
/// How long a request waits for another's miss on the same key.
const LOCK_WAIT: Duration = Duration::from_secs(5);
/// Keys remembered as uncacheable, per predictor shard.
const PREDICTED_KEYS: usize = 4096;

/// A cache policy as requests use it.
pub(crate) struct CachePlan {
    pub policy: CachePolicy,
    key: Template,
    vary: Vec<HeaderName>,
    bypass: CompiledConditions,
}

impl CachePlan {
    pub(crate) fn compile(policy: &CachePolicy) -> Result<Self> {
        let invalid = |detail: String| {
            PanelError::validation_failed(format!("cache policy {} {detail}", policy.id))
        };
        Ok(Self {
            key: Template::parse(policy.key())
                .map_err(|error| invalid(format!("has a key that is not a template: {error}")))?,
            vary: policy
                .vary_headers
                .iter()
                .map(|name| {
                    HeaderName::from_bytes(name.as_bytes())
                        .map_err(|_| invalid(format!("varies by {name:?}, which is not a field")))
                })
                .collect::<Result<_>>()?,
            bypass: CompiledConditions::compile(&policy.bypass)
                .map_err(|error| invalid(format!("bypasses requests that {}", error.message)))?,
            policy: policy.clone(),
        })
    }

    /// Whether `request` neither uses nor fills the cache.
    pub(crate) fn bypasses(&self, request: &impl Request) -> bool {
        self.bypass.any_holds(request)
    }

    pub(crate) fn key(&self, facts: &Facts<'_>) -> Vec<u8> {
        self.key.render(facts).to_vec()
    }

    /// The variance of `request` for a response stored with `meta`: the
    /// fields its `Vary` names and the policy's own.
    pub(crate) fn variance(&self, meta: &CacheMeta, request: &RequestHeader) -> Option<HashBinary> {
        let mut names: Vec<String> = self
            .vary
            .iter()
            .map(|name| name.as_str().to_owned())
            .collect();
        for value in meta.response_header().headers.get_all(header::VARY) {
            if let Ok(value) = value.to_str() {
                names.extend(
                    value
                        .split(',')
                        .map(|name| name.trim().to_ascii_lowercase())
                        .filter(|name| !name.is_empty()),
                );
            }
        }
        names.sort();
        names.dedup();
        let mut variance = VarianceBuilder::new();
        for name in names {
            let mut value = Vec::new();
            for (index, line) in request.headers.get_all(name.as_str()).iter().enumerate() {
                if index > 0 {
                    value.push(b',');
                }
                value.extend_from_slice(line.as_bytes());
            }
            variance.add_owned_name_value(name, value);
        }
        variance.finalize()
    }

    /// Whether and for how long `response` is stored, for a request with or
    /// without `Authorization` (RFC 9111 §3, §4.2, §5.2.2).
    pub(crate) fn decide(&self, response: &ResponseHeader, authorization: bool) -> RespCacheable {
        let uncacheable = |reason| RespCacheable::Uncacheable(NoCacheReason::Custom(reason));
        let status = response.status.as_u16();
        if response.headers.contains_key(header::SET_COOKIE) {
            return uncacheable("set-cookie");
        }
        if status == 206 {
            return uncacheable("partial");
        }
        let any_variant = response
            .headers
            .get_all(header::VARY)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .any(|value| value.split(',').any(|name| name.trim() == "*"));
        if any_variant {
            return uncacheable("vary");
        }
        if self.policy.refuses(status) {
            return uncacheable("status");
        }
        let control = CacheControl::from_resp_headers(response);
        if authorization
            && !control
                .as_ref()
                .is_some_and(|control| control.allow_caching_authorized_req())
        {
            return uncacheable("authorization");
        }
        let honored = control.as_ref().filter(|_| self.policy.honor_origin);
        if honored.is_some_and(|control| control.is_cacheable() == Cacheable::No) {
            return RespCacheable::Uncacheable(NoCacheReason::OriginNotCache);
        }
        let now = SystemTime::now();
        let fresh = |duration: Duration| {
            if duration.is_zero() {
                now.checked_sub(Duration::from_secs(1))
            } else {
                now.checked_add(duration)
            }
        };
        let origin = if self.policy.honor_origin {
            honored
                .and_then(|control| control.fresh_duration())
                .and_then(fresh)
                .or_else(|| calculate_expires_header_time(response))
        } else {
            None
        };
        let Some(fresh_until) = origin.or_else(|| {
            self.policy
                .fresh_seconds(status)
                .and_then(|seconds| fresh(Duration::from_secs(seconds)))
        }) else {
            return RespCacheable::Uncacheable(NoCacheReason::OriginNotCache);
        };
        let seconds = |duration: Duration| u32::try_from(duration.as_secs()).unwrap_or(u32::MAX);
        let while_revalidating = honored
            .and_then(|control| control.serve_stale_while_revalidate_duration())
            .map_or(self.policy.stale_while_revalidate_seconds, seconds);
        let if_error = honored
            .and_then(|control| control.serve_stale_if_error_duration())
            .map_or(self.policy.stale_if_error_seconds, seconds);
        let mut stored = response.clone();
        if let Some(control) = honored {
            control.strip_private_headers(&mut stored);
        }
        RespCacheable::Cacheable(CacheMeta::new(
            fresh_until,
            now,
            while_revalidating,
            if_error,
            stored,
        ))
    }
}

/// What the cache did for a request.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) enum Outcome {
    Hit,
    Stale,
    Updating,
    Miss,
    Expired,
    Revalidated,
    Bypass,
    Uncacheable,
}

impl Outcome {
    pub(crate) const ALL: [Self; 8] = [
        Self::Hit,
        Self::Stale,
        Self::Updating,
        Self::Miss,
        Self::Expired,
        Self::Revalidated,
        Self::Bypass,
        Self::Uncacheable,
    ];

    /// The outcome of a request whose cache is in `phase`; `None` when the
    /// cache had no part in it.
    pub(crate) fn of(phase: &CachePhase, bypassed: bool) -> Option<Self> {
        if bypassed {
            return Some(Self::Bypass);
        }
        Some(match phase {
            CachePhase::Hit => Self::Hit,
            CachePhase::Stale => Self::Stale,
            CachePhase::StaleUpdating => Self::Updating,
            CachePhase::Miss => Self::Miss,
            CachePhase::Expired => Self::Expired,
            CachePhase::Revalidated | CachePhase::RevalidatedNoCache(_) => Self::Revalidated,
            CachePhase::Bypass => Self::Uncacheable,
            CachePhase::Disabled(NoCacheReason::NeverEnabled) => return None,
            CachePhase::Disabled(_) => Self::Uncacheable,
            CachePhase::Uninit | CachePhase::CacheKey => return None,
        })
    }

    /// nginx's `$upstream_cache_status`.
    pub(crate) fn variable(self) -> &'static str {
        match self {
            Self::Hit => "HIT",
            Self::Stale => "STALE",
            Self::Updating => "UPDATING",
            Self::Miss | Self::Uncacheable => "MISS",
            Self::Expired => "EXPIRED",
            Self::Revalidated => "REVALIDATED",
            Self::Bypass => "BYPASS",
        }
    }

    /// The parameters of this cache's `Cache-Status` entry (RFC 9211 §2).
    pub(crate) fn status(self) -> &'static str {
        match self {
            Self::Hit => "hit",
            Self::Stale | Self::Updating => "hit; fwd=stale",
            Self::Miss => "fwd=miss; stored",
            Self::Uncacheable => "fwd=miss",
            Self::Expired => "fwd=stale; stored",
            Self::Revalidated => "fwd=stale; fwd-status=304",
            Self::Bypass => "fwd=bypass",
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Hit => "hit",
            Self::Stale => "stale",
            Self::Updating => "updating",
            Self::Miss => "miss",
            Self::Expired => "expired",
            Self::Revalidated => "revalidated",
            Self::Bypass => "bypass",
            Self::Uncacheable => "uncacheable",
        }
    }
}

fn overrides() -> CacheOptionOverrides {
    let mut overrides = CacheOptionOverrides::default();
    overrides.wait_timeout = Some(LOCK_WAIT);
    overrides
}

/// What a request does with the cache of its route.
pub(crate) struct RequestCache {
    pub plan: Arc<CachePlan>,
    pub site: String,
    /// The primary key the policy rendered; `None` when the request
    /// bypasses the cache.
    pub primary: Option<Vec<u8>>,
}

/// The name this cache goes by in `Cache-Status`.
pub(crate) const CACHE_NAME: &str = "pingora-panel";

/// What the cache did for a site since the gateway started.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct SiteCounts(pub BTreeMap<Outcome, u64>);

/// What the proxy cache holds and did since the gateway started.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CacheReport {
    pub bytes: u64,
    pub entries: u64,
    pub max_bytes: u64,
    pub since: SystemTime,
    pub sites: Vec<SiteCacheReport>,
}

/// Requests of a site by what the cache did for them: `hit`, `stale`,
/// `updating`, `miss`, `expired`, `revalidated`, `bypass` and
/// `uncacheable`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SiteCacheReport {
    pub site_id: String,
    pub outcomes: Vec<(&'static str, u64)>,
}

/// What to purge from the proxy cache.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CachePurge {
    All,
    Sites(Vec<String>),
    /// Absolute URLs of the active configuration's sites.
    Urls(Vec<String>),
}

/// The keys `urls` are stored under: one for each cache plan of the site
/// serving its host, rendered from its scheme, host and target.
pub(crate) fn url_keys(
    routing: &crate::routing::RoutingTable,
    urls: &[String],
) -> Result<Vec<(String, Vec<u8>)>> {
    let mut keys = Vec::new();
    for url in urls {
        let refused = || {
            PanelError::invalid_argument(format!("{url:?} is not an http or https URL with a host"))
        };
        let uri: http::Uri = url.parse().map_err(|_| refused())?;
        let scheme = uri
            .scheme_str()
            .filter(|scheme| matches!(*scheme, "http" | "https"))
            .ok_or_else(refused)?;
        let host = uri.host().ok_or_else(refused)?.to_ascii_lowercase();
        let entry = routing
            .lookup(&host)
            .ok_or_else(|| PanelError::not_found(format!("no site serves {host}")))?;
        let site = routing.site(entry.site);
        let path = panel_routing::path::normalize(uri.path())
            .map_or_else(|| uri.path().to_owned(), |path| path.into_owned());
        let headers = http::HeaderMap::new();
        let variables = HashMap::new();
        let facts = Facts {
            host: &host,
            uri: &path,
            query: uri.query(),
            request_uri: uri.path_and_query().map_or("/", |target| target.as_str()),
            method: "GET",
            scheme,
            client_ip: None,
            headers: &headers,
            upstream: None,
            cache_status: None,
            variables: &variables,
        };
        let mut rendered: Vec<*const CachePlan> = Vec::new();
        for plan in site
            .routes()
            .iter()
            .filter_map(|route| route.cache.as_ref())
        {
            if !rendered.contains(&Arc::as_ptr(plan)) {
                rendered.push(Arc::as_ptr(plan));
                keys.push((site.id.to_string(), plan.key(&facts)));
            }
        }
    }
    Ok(keys)
}

/// What to purge.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PurgeScope {
    All,
    Sites(Vec<String>),
    /// Primary keys as policies render them, by site.
    Keys(Vec<(String, Vec<u8>)>),
}

/// The cache of a gateway: one store, lock and predictor for every
/// snapshot it serves.
pub(crate) struct Cache {
    pub store: CacheStore,
    lock: Box<CacheKeyLockImpl>,
    predictor: Predictor<16>,
    /// Tells apart gateways sharing a process, as tests do.
    instance: u64,
    generation: AtomicU64,
    sites: Mutex<HashMap<String, u64>>,
    counts: Mutex<HashMap<String, SiteCounts>>,
    pub started: SystemTime,
}

impl Cache {
    /// A cache living as long as the process, as Pingora's cache needs.
    pub(crate) fn leak(max_bytes: u64) -> &'static Self {
        let mut instance = [0; 8];
        getrandom::fill(&mut instance).expect("the system has randomness");
        Box::leak(Box::new(Self {
            store: CacheStore::new(max_bytes),
            lock: CacheLock::new_boxed(LOCK_AGE),
            predictor: Predictor::new(PREDICTED_KEYS, None),
            instance: u64::from_le_bytes(instance),
            generation: AtomicU64::new(0),
            sites: Mutex::new(HashMap::new()),
            counts: Mutex::new(HashMap::new()),
            started: SystemTime::now(),
        }))
    }

    /// Lets `session` use the cache under `plan`.
    pub(crate) fn enable(&'static self, session: &mut Session, plan: &CachePlan) {
        session.cache.enable(
            &self.store,
            None,
            Some(&self.predictor),
            Some(&*self.lock),
            Some(overrides()),
        );
        session.cache.set_max_file_size_bytes(
            usize::try_from(plan.policy.max_object_bytes()).unwrap_or(usize::MAX),
        );
    }

    /// The key of `primary` for `site`, its components framed by their
    /// lengths so none can run into the next.
    pub(crate) fn key(&self, site: &str, primary: &[u8]) -> CacheKey {
        let site_generation = self.sites.lock().get(site).copied().unwrap_or(0);
        let mut framed = Vec::with_capacity(primary.len() + site.len() + 40);
        for part in [
            &self.instance.to_be_bytes()[..],
            &self.generation.load(Ordering::Acquire).to_be_bytes(),
            site.as_bytes(),
            &site_generation.to_be_bytes(),
            primary,
        ] {
            framed.extend_from_slice(&u32::try_from(part.len()).unwrap_or(u32::MAX).to_be_bytes());
            framed.extend_from_slice(part);
        }
        CacheKey::new(framed, "")
    }

    pub(crate) fn purge(&self, scope: &PurgeScope) {
        match scope {
            PurgeScope::All => {
                self.generation.fetch_add(1, Ordering::AcqRel);
            }
            PurgeScope::Sites(sites) => {
                let mut generations = self.sites.lock();
                for site in sites {
                    *generations.entry(site.clone()).or_default() += 1;
                }
            }
            PurgeScope::Keys(keys) => {
                let keys: Vec<_> = keys
                    .iter()
                    .map(|(site, primary)| self.key(site, primary).primary_bin())
                    .collect();
                self.store.purge_keys(keys);
            }
        }
    }

    pub(crate) fn count(&self, site: &str, outcome: Outcome) {
        let mut counts = self.counts.lock();
        let site = match counts.get_mut(site) {
            Some(site) => site,
            None => counts.entry(site.to_owned()).or_default(),
        };
        *site.0.entry(outcome).or_default() += 1;
    }

    pub(crate) fn counts(&self) -> BTreeMap<String, SiteCounts> {
        self.counts
            .lock()
            .iter()
            .map(|(site, counts)| (site.clone(), counts.clone()))
            .collect()
    }

    pub(crate) fn report(&self) -> CacheReport {
        let usage: StoreUsage = self.store.usage();
        CacheReport {
            bytes: usage.bytes,
            entries: usage.entries,
            max_bytes: usage.max_bytes,
            since: self.started,
            sites: self
                .counts()
                .into_iter()
                .map(|(site_id, counts)| SiteCacheReport {
                    site_id,
                    outcomes: Outcome::ALL
                        .iter()
                        .map(|outcome| {
                            (outcome.name(), counts.0.get(outcome).copied().unwrap_or(0))
                        })
                        .collect(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(adjust: impl FnOnce(&mut CachePolicy)) -> CachePlan {
        let mut policy = CachePolicy::new("pages");
        policy.ttl_seconds = 600;
        adjust(&mut policy);
        CachePlan::compile(&policy).unwrap()
    }

    fn response(status: u16, headers: &[(&str, &str)]) -> ResponseHeader {
        let mut response = ResponseHeader::build(status, None).unwrap();
        for (name, value) in headers {
            response.append_header(name.to_string(), *value).unwrap();
        }
        response
    }

    fn fresh_for(decision: RespCacheable) -> Option<i64> {
        match decision {
            RespCacheable::Cacheable(meta) => Some(
                meta.fresh_until()
                    .duration_since(SystemTime::now())
                    .map_or(-1, |left| i64::try_from(left.as_secs()).unwrap() + 1),
            ),
            RespCacheable::Uncacheable(_) => None,
        }
    }

    #[test]
    fn the_origin_decides_first_then_the_policy() {
        let plan = plan(|_| {});
        assert!(fresh_for(plan.decide(&response(200, &[]), false)).is_some_and(|s| s > 590));
        let origin = response(200, &[("cache-control", "max-age=30")]);
        assert!(fresh_for(plan.decide(&origin, false)).is_some_and(|s| (25..=31).contains(&s)));
        for refused in [
            response(200, &[("cache-control", "no-store")]),
            response(200, &[("cache-control", "private")]),
            response(200, &[("set-cookie", "session=1")]),
            response(200, &[("vary", "accept, *")]),
            response(206, &[]),
            response(500, &[]),
        ] {
            assert_eq!(fresh_for(plan.decide(&refused, false)), None, "{refused:?}");
        }
        assert_eq!(
            fresh_for(plan.decide(&response(200, &[("cache-control", "no-cache")]), false)),
            Some(-1),
            "no-cache stores a response stale, to revalidate it first"
        );
        let expires = response(200, &[("expires", "Thu, 01 Jan 1970 00:00:00 GMT")]);
        assert_eq!(fresh_for(plan.decide(&expires, false)), Some(-1));
    }

    #[test]
    fn authorization_needs_the_response_to_allow_sharing() {
        let plan = plan(|_| {});
        assert_eq!(fresh_for(plan.decide(&response(200, &[]), true)), None);
        let shared = response(200, &[("cache-control", "public, max-age=60")]);
        assert!(fresh_for(plan.decide(&shared, true)).is_some());
    }

    #[test]
    fn a_policy_that_overrides_the_origin_keeps_its_own_times() {
        let plan = plan(|policy| {
            policy.honor_origin = false;
            policy.status_ttls = [(404, 30), (200, 0)].into();
        });
        let private = response(404, &[("cache-control", "private, max-age=9000")]);
        assert!(fresh_for(plan.decide(&private, false)).is_some_and(|s| (25..=31).contains(&s)));
        assert_eq!(fresh_for(plan.decide(&response(200, &[]), false)), None);
        assert_eq!(
            fresh_for(plan.decide(&response(404, &[("set-cookie", "a=1")]), false)),
            None,
            "Set-Cookie is never stored"
        );
    }

    #[test]
    fn variants_follow_vary_and_the_policys_fields() {
        let plan = plan(|policy| policy.vary_headers = ["x-tenant".to_owned()].into());
        let now = SystemTime::now();
        let meta = CacheMeta::new(
            now,
            now,
            0,
            0,
            response(200, &[("vary", "Accept-Encoding")]),
        );
        let request = |encoding: &str, tenant: &str| {
            let mut request = RequestHeader::build("GET", b"/", None).unwrap();
            request.insert_header("accept-encoding", encoding).unwrap();
            request.insert_header("x-tenant", tenant).unwrap();
            request
        };
        let gzip = plan.variance(&meta, &request("gzip", "a"));
        assert!(gzip.is_some());
        assert_eq!(gzip, plan.variance(&meta, &request("gzip", "a")));
        assert_ne!(gzip, plan.variance(&meta, &request("br", "a")));
        assert_ne!(gzip, plan.variance(&meta, &request("gzip", "b")));
    }

    #[test]
    fn keys_tell_sites_and_generations_apart() {
        let cache = Cache::leak(1 << 20);
        let shop = cache.key("shop", b"https://shop.example/");
        assert_eq!(
            shop.primary_bin(),
            cache.key("shop", b"https://shop.example/").primary_bin()
        );
        assert_ne!(
            shop.primary_bin(),
            cache.key("blog", b"https://shop.example/").primary_bin()
        );
        cache.purge(&PurgeScope::Sites(vec!["shop".into()]));
        assert_ne!(
            shop.primary_bin(),
            cache.key("shop", b"https://shop.example/").primary_bin()
        );
        let blog = cache.key("blog", b"/");
        cache.purge(&PurgeScope::All);
        assert_ne!(blog.primary_bin(), cache.key("blog", b"/").primary_bin());
        cache.count("shop", Outcome::Hit);
        cache.count("shop", Outcome::Hit);
        cache.count("shop", Outcome::Miss);
        assert_eq!(cache.counts()["shop"].0[&Outcome::Hit], 2);
    }

    #[test]
    fn outcomes_follow_the_cache_phase() {
        assert_eq!(Outcome::of(&CachePhase::Hit, false), Some(Outcome::Hit));
        assert_eq!(Outcome::of(&CachePhase::Hit, true), Some(Outcome::Bypass));
        assert_eq!(
            Outcome::of(&CachePhase::Disabled(NoCacheReason::NeverEnabled), false),
            None
        );
        assert_eq!(
            Outcome::of(&CachePhase::Disabled(NoCacheReason::OriginNotCache), false),
            Some(Outcome::Uncacheable)
        );
        assert_eq!(Outcome::Miss.status(), "fwd=miss; stored");
        assert_eq!(Outcome::Revalidated.variable(), "REVALIDATED");
    }
}
