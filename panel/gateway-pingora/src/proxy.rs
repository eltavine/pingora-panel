//! The request path: host and site resolution, redirects, route actions and
//! upstream selection against the active snapshot.

use crate::{
    access_log::{self, AccessPlan, LoggingPlan, Served},
    acme::ChallengeDirectory,
    adapter::{ActiveSnapshot, PreparedPingoraSnapshot},
    cache::{Outcome, RequestCache, CACHE_NAME},
    certificates::{ChosenCertificates, Handshake},
    error_pages::Prepared,
    forwarding::{self, Forwarding},
    head_deadline::Connections,
    hosts::{self, HostError, RequestHost},
    hsts::{StrictTransport, StrictTransportBuilder},
    http_policy::{self, FieldChange, HttpPolicy, HttpPolicyBuilder, HttpPolicyModule},
    log_files::{Destination, Logs},
    request_identity,
    resilience::Busy,
    responses,
    rewrite::{InternalTarget, Rewrites, Rewritten},
    routing::{CompiledRoute, RouteTarget, RoutingTable, SiteRoutes},
    security::{Admission, Candidate, ClientResolution, Refusal},
    static_files,
    telemetry::{self, GatewayMetrics},
    template::Facts,
    upstream::{EndpointLease, UpstreamPool},
};
use async_trait::async_trait;
use chrono::Utc;
use http::{header, HeaderValue};
use panel_ir::AccessLogFormat;
use panel_metrics::{method, protocol_version, ActiveRequest, ClientRequest, ServerRequest};
use pingora_cache::{key::HashBinary, CacheKey, CacheMeta, NoCacheReason, RespCacheable};
use pingora_core::{
    modules::http::{compression::ResponseCompressionBuilder, HttpModules},
    protocols::http::v1::common::is_upgrade_req,
    upstreams::peer::HttpPeer,
    Error, ErrorSource, ErrorType,
};
use pingora_http::{RequestHeader, ResponseHeader};
use pingora_proxy::{FailToProxy, ProxyHttp, Session};
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        atomic::{AtomicUsize, Ordering::Relaxed},
        Arc,
    },
    time::Instant,
};

mod handshake;
mod lua_phases;

pub(crate) use handshake::{HandshakeScripts, Sessions};
use lua_phases::LuaStep;

/// RFC 9211's field.
const CACHE_STATUS: http::HeaderName = http::HeaderName::from_static("cache-status");

const REDIRECT_STATUS: u16 = 308;
/// Times the rewrite phase may send a request to another route, as nginx
/// allows ten internal redirects.
/// URI changes a request may make, by jumps and internal redirects together,
/// as nginx allows.
const MOST_URI_CHANGES: usize = 10;
/// Forwarding headers dropped from peers that are not trusted proxies, so
/// upstreams are not told a forged client.
const UNTRUSTED_FORWARDING: [&str; 3] = ["x-forwarded-for", "x-real-ip", "forwarded"];
static X_REAL_IP: header::HeaderName = header::HeaderName::from_static("x-real-ip");

/// Answers a request a security policy refused.
async fn refuse(session: &mut Session, refusal: Refusal) -> pingora_core::Result<()> {
    let mut headers: Vec<(header::HeaderName, &str)> = refusal
        .headers
        .iter()
        .map(|(name, value)| (name.clone(), value.as_str()))
        .collect();
    match &refusal.custom {
        Some((body, content_type)) => {
            headers.push((
                header::CONTENT_TYPE,
                content_type
                    .as_deref()
                    .unwrap_or("text/plain; charset=utf-8"),
            ));
            headers.push((header::CACHE_CONTROL, "no-store"));
            responses::send(
                session,
                refusal.status,
                &headers,
                bytes::Bytes::from(body.clone()),
            )
            .await
        }
        None => responses::plain(session, refusal.status, &refusal.message, &headers).await,
    }
}

/// What a listener contributes to each request it accepts.
pub(crate) struct ListenerContext {
    pub id: String,
    pub tls: bool,
    pub http1: bool,
    pub challenges: Option<Arc<ChallengeDirectory>>,
    pub client: ClientResolution,
    /// The listener's connections, told when a request on them is done.
    pub connections: Arc<Connections>,
    /// Where requests are measured, when the gateway is.
    pub metrics: Option<GatewayMetrics>,
    /// Where requests are logged, when the gateway has a log directory.
    pub logs: Option<Logs>,
}

pub(crate) struct PanelProxy {
    listener: Arc<ListenerContext>,
    active: ActiveSnapshot,
    in_flight: Arc<AtomicUsize>,
}

impl PanelProxy {
    pub(crate) fn new(
        listener: ListenerContext,
        active: ActiveSnapshot,
        in_flight: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            listener: Arc::new(listener),
            active,
            in_flight,
        }
    }

    /// The sessions the listener resumes, which its session scripts fetch
    /// and store.
    pub(crate) fn sessions(&self) -> Arc<Sessions> {
        Arc::new(Sessions::new(
            Arc::clone(&self.listener),
            Arc::clone(&self.active),
        ))
    }

    /// What runs the handshake scripts of the listener's sites, presenting
    /// the certificates they choose through `chosen` and resuming the
    /// sessions they find from `sessions`.
    pub(crate) fn handshake_scripts(
        &self,
        chosen: Arc<ChosenCertificates>,
        sessions: Arc<Sessions>,
    ) -> HandshakeScripts {
        HandshakeScripts {
            listener: Arc::clone(&self.listener),
            active: Arc::clone(&self.active),
            chosen,
            sessions,
        }
    }
}

struct InFlight(Arc<AtomicUsize>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Relaxed);
    }
}

pub(crate) struct RequestContext {
    _in_flight: InFlight,
    snapshot: Option<Arc<PreparedPingoraSnapshot>>,
    site: Option<usize>,
    /// The configured domain the site was found by, if it was.
    domain: Option<usize>,
    route: Option<usize>,
    pool: Option<usize>,
    tried: Vec<usize>,
    endpoint: Option<usize>,
    lease: Option<EndpointLease>,
    sent_at: Option<Instant>,
    failure_recorded: bool,
    /// The client, after trusted proxies.
    client: Option<IpAddr>,
    /// Whether the peer is a trusted proxy whose forwarding headers count.
    trusted_peer: bool,
    /// What security policies left the request to keep to.
    admission: Admission,
    /// Request field changes of its HTTP policies, rendered for it.
    http_request: Vec<FieldChange>,
    /// The HTTP policy whose compression applies to its responses.
    compression: Option<usize>,
    /// The client's `Accept-Encoding` lines, while the compression module
    /// reads ranked ones.
    accept_encoding: Option<Vec<HeaderValue>>,
    /// Whether the request is a trial of a half-open circuit whose outcome
    /// is not counted yet.
    trial: bool,
    /// The request's place among those its upstream takes at once.
    place: Option<tokio::sync::OwnedSemaphorePermit>,
    /// Retries made under the upstream's retry policy.
    retries: u32,
    /// Whether the upstream's response is being replaced by a retry.
    retrying: bool,
    body_seen: u64,
    started: Instant,
    /// Counts the request as active while it is measured.
    active: Option<ActiveRequest>,
    /// A body a script set, sent upstream instead of the client's.
    lua_body: Option<bytes::Bytes>,
    /// A body filter ended the response before the upstream did.
    lua_body_done: bool,
    /// Tries the balancer allows beyond the upstream's own.
    lua_more_tries: u32,
    /// Tries the balancer chose for.
    lua_tries: u32,
    /// How the last try the balancer chose ended, for `get_last_failure`.
    lua_last_failure: Option<(String, Option<u16>)>,
    /// Where the balancer sent the current try.
    lua_peer: Option<SocketAddr>,
    /// The variables `set` gave the request before it had scripts; the
    /// scripts hold them once they do.
    variables: HashMap<String, String>,
    /// What a script's subrequest keeps of the script that made it.
    subrequest: Option<crate::subrequests::Subrequest>,
    /// The named location `ngx.exec("@name")` sent the request to.
    named: Option<String>,
    /// The client's request target, which rewrites leave for
    /// `$request_uri`.
    request_uri: String,
    /// Whether a rewrite, an internal redirect or a script sent the request
    /// where it is, so an internal route takes it.
    internal: bool,
    /// The requested host, once it is known, for error pages.
    host: String,
    scheme: &'static str,
    /// An error page's body in place of the upstream's, until it is sent.
    replacement: Option<Option<bytes::Bytes>>,
    /// The pool the route proxies to, whether the cache answers or not.
    upstream_target: Option<usize>,
    /// The cache of the route's requests (ADR 0043).
    cache: Option<RequestCache>,
    /// What the cache did, once the response is known.
    cache_outcome: Option<Outcome>,
}

impl RequestContext {
    fn upstream(&self) -> Option<(&UpstreamPool, usize)> {
        let pool = &self.snapshot.as_ref()?.pools[self.pool?];
        Some((pool, self.endpoint?))
    }

    /// Counts an attempt's outcome for its endpoint and upstream; the first
    /// one settles a circuit trial.
    fn record_outcome(&mut self, failed: bool) {
        let trial = std::mem::take(&mut self.trial);
        if let Some((pool, endpoint)) = self.upstream() {
            if failed {
                pool.record_failure(endpoint, trial);
            } else {
                pool.record_success(endpoint, trial);
            }
        }
        self.failure_recorded = true;
    }

    /// Whether the request could be sent again as it was: idempotent
    /// (RFC 9110 §9.2.2), its body still held, and nothing sent back yet.
    fn replayable(session: &Session) -> bool {
        session.req_header().method.is_idempotent()
            && !session.as_ref().retry_buffer_truncated()
            && session.response_written().is_none()
    }
}

/// What a site's or route's rules did to a request.
enum Rewrite {
    /// The request was answered, with a redirect or an error.
    Answered,
    Unchanged,
    Changed {
        path: String,
        /// A route is chosen again.
        reroute: bool,
    },
}

/// A request as routing sees it: its normalized host and path, and the
/// client after trusted proxies.
struct Routed<'a> {
    header: &'a RequestHeader,
    host: &'a str,
    path: &'a str,
    client: Option<IpAddr>,
}

impl panel_routing::Request for Routed<'_> {
    fn method(&self) -> &str {
        self.header.method.as_str()
    }

    fn host(&self) -> &str {
        self.host
    }

    fn path(&self) -> &str {
        self.path
    }

    fn query(&self) -> Option<&str> {
        self.header.uri.query()
    }

    fn header_lines(&self, name: &str) -> impl Iterator<Item = &[u8]> {
        self.header
            .headers
            .get_all(name)
            .into_iter()
            .map(http::HeaderValue::as_bytes)
    }

    fn client(&self) -> Option<IpAddr> {
        self.client
    }
}

#[async_trait]
impl ProxyHttp for PanelProxy {
    type CTX = RequestContext;

    fn init_downstream_modules(&self, modules: &mut HttpModules) {
        modules.add_module(ResponseCompressionBuilder::enable(1));
        modules.add_module(Box::new(StrictTransportBuilder));
        modules.add_module(Box::new(HttpPolicyBuilder));
        modules.add_module(Box::new(lua_phases::LuaModuleBuilder));
    }

    fn new_ctx(&self) -> RequestContext {
        self.in_flight.fetch_add(1, Relaxed);
        RequestContext {
            _in_flight: InFlight(Arc::clone(&self.in_flight)),
            snapshot: None,
            site: None,
            domain: None,
            route: None,
            pool: None,
            tried: Vec::new(),
            endpoint: None,
            lease: None,
            sent_at: None,
            failure_recorded: false,
            client: None,
            trusted_peer: false,
            admission: Admission::default(),
            http_request: Vec::new(),
            compression: None,
            accept_encoding: None,
            trial: false,
            place: None,
            retries: 0,
            retrying: false,
            body_seen: 0,
            started: Instant::now(),
            active: None,
            lua_body: None,
            lua_body_done: false,
            lua_more_tries: 0,
            lua_tries: 0,
            lua_last_failure: None,
            lua_peer: None,
            variables: HashMap::new(),
            subrequest: None,
            named: None,
            request_uri: String::new(),
            internal: false,
            host: String::new(),
            scheme: "http",
            replacement: None,
            upstream_target: None,
            cache: None,
            cache_outcome: None,
        }
    }

    /// Scripts make subrequests with Pingora's, which take the request's
    /// session.
    fn allow_spawning_subrequest(&self, _session: &Session, _ctx: &RequestContext) -> bool {
        true
    }

    async fn early_request_filter(
        &self,
        session: &mut Session,
        ctx: &mut RequestContext,
    ) -> pingora_core::Result<()> {
        ctx.accept_encoding = http_policy::rank_codings(session.req_header_mut());
        if let Some((subrequest, variables)) = crate::subrequests::begin(session) {
            ctx.subrequest = Some(subrequest);
            ctx.variables = variables;
        }
        Ok(())
    }

    async fn request_filter(
        &self,
        session: &mut Session,
        ctx: &mut RequestContext,
    ) -> pingora_core::Result<bool> {
        if let Some(original) = ctx.accept_encoding.take() {
            http_policy::restore_codings(session.req_header_mut(), original);
        }
        http_policy::disable_compression(session);
        request_identity::ensure_request_id(session.req_header_mut());
        ctx.scheme = self.scheme();
        if ctx.request_uri.is_empty() {
            ctx.request_uri = session
                .req_header()
                .uri
                .path_and_query()
                .map_or("/", |target| target.as_str())
                .to_owned();
        }
        if let Some(metrics) = self
            .listener
            .metrics
            .as_ref()
            .filter(|_| ctx.subrequest.is_none())
        {
            ctx.active = Some(
                metrics
                    .server
                    .start(method(&session.req_header().method), self.scheme()),
            );
        }
        let Some(snapshot) = self.active.load_full() else {
            responses::plain(
                session,
                503,
                "no configuration is active",
                &[(header::RETRY_AFTER, "5")],
            )
            .await?;
            return Ok(true);
        };
        ctx.snapshot = Some(Arc::clone(&snapshot));
        if !self.listener.http1 && !session.is_http2() {
            responses::plain(session, 505, "this listener requires HTTP/2", &[]).await?;
            return Ok(true);
        }
        if let Some(peer) = client_address(session).map(|address| address.ip()) {
            let (client, trusted) = self.listener.client.resolve(peer, session.req_header());
            ctx.client = Some(client);
            ctx.trusted_peer = trusted;
        }
        if let Some(key_authorization) = self.challenge_answer(session).await {
            responses::send(
                session,
                200,
                &[
                    (header::CONTENT_TYPE, "application/octet-stream"),
                    (header::CACHE_CONTROL, "no-store"),
                ],
                key_authorization,
            )
            .await?;
            return Ok(true);
        }
        let host = match hosts::request_host(session.req_header()) {
            Ok(host) => host,
            Err(error) => {
                let message = match error {
                    HostError::Missing => "the request has no Host",
                    HostError::Multiple => "the request has more than one Host",
                    HostError::Invalid => "the request host is invalid",
                };
                responses::plain(session, 400, message, &[]).await?;
                return Ok(true);
            }
        };
        let host_name = host.as_ref().map_or("", |host| host.name.as_str());
        ctx.host = host_name.to_owned();
        if let Some(rejection) = self.tls_rejection(session, &snapshot, host_name) {
            let (status, message) = rejection;
            responses::plain(session, status, message, &[]).await?;
            return Ok(true);
        }

        let routing = &snapshot.routing;
        let entry = routing
            .lookup(host_name)
            .filter(|entry| routing.serves(entry.site, &self.listener.id));
        ctx.domain = entry.map(|entry| entry.domain);
        let (site_index, alias) = match entry {
            Some(entry) => (entry.site, entry.redirect_to_primary),
            None => match routing.default_site(&self.listener.id) {
                Some(index) => (index, false),
                None => {
                    responses::plain(session, 421, "no site serves this host here", &[]).await?;
                    return Ok(true);
                }
            },
        };
        ctx.site = Some(site_index);
        let site = routing.site(site_index);
        if self.listener.tls && site.hsts.is_some() {
            if let Some(module) = session.downstream_modules_ctx.get_mut::<StrictTransport>() {
                module.header.clone_from(&site.hsts);
            }
        }
        if let Some(location) = site_redirect(
            site,
            host.as_ref(),
            alias,
            self.listener.tls,
            session.req_header(),
        ) {
            responses::redirect(session, REDIRECT_STATUS, &location).await?;
            return Ok(true);
        }
        // A script's subrequest belongs to a request the site admitted.
        if let Some(maintenance) = site
            .maintenance
            .as_ref()
            .filter(|maintenance| ctx.subrequest.is_none() && !maintenance.admits(ctx.client))
        {
            let retry_after = maintenance.retry_after.as_deref();
            let headers: Vec<(header::HeaderName, &str)> = retry_after
                .map(|seconds| (header::RETRY_AFTER, seconds))
                .into_iter()
                .collect();
            match &maintenance.body {
                Some(body) => {
                    let rendered =
                        lua_phases::with_variables(session, &ctx.variables, |variables| {
                            let path = session.req_header().uri.path();
                            body.render(&facts(
                                session,
                                host_name,
                                path,
                                self.listener.tls,
                                variables,
                                &ctx.request_uri,
                            ))
                        });
                    let mut all = headers.clone();
                    all.push((
                        header::CONTENT_TYPE,
                        maintenance
                            .content_type
                            .as_deref()
                            .unwrap_or("text/plain; charset=utf-8"),
                    ));
                    responses::send(session, maintenance.status, &all, rendered).await?;
                }
                None => {
                    Self::error(
                        session,
                        ctx,
                        maintenance.status,
                        "the site is under maintenance",
                        &headers,
                    )
                    .await?;
                }
            }
            return Ok(true);
        }
        let mut changes_left = MOST_URI_CHANGES;
        'request: loop {
            // A named location starts at its own rewrite phase, as nginx's
            // does: the server's variables, rules and handler do not run
            // again.
            let named = ctx.named.take();
            let mut site_path = panel_routing::path::normalize(session.req_header().uri.path())
                .map_or_else(
                    || session.req_header().uri.path().to_owned(),
                    |path| path.into_owned(),
                );
            self.lua_prepare_header_filter(session, ctx, &site_path, host_name);
            let site_variables = if named.is_some() {
                &[][..]
            } else {
                &site.lua.variables[..]
            };
            if let LuaStep::Done = self
                .lua_variables(session, ctx, site_variables, &site_path, host_name)
                .await?
            {
                return Ok(true);
            }
            if named.is_none() && !site.rewrites.is_empty() {
                match self
                    .rewrite(session, ctx, &site.rewrites, &site_path, host_name)
                    .await?
                {
                    Rewrite::Answered => return Ok(true),
                    Rewrite::Unchanged => {}
                    Rewrite::Changed { path, .. } => site_path = path,
                }
            }
            if let Some(hook) = site.lua.server_rewrite.clone().filter(|_| named.is_none()) {
                match self
                    .lua_request(session, ctx, &hook, &site_path, host_name, true)
                    .await?
                {
                    LuaStep::Done => return Ok(true),
                    LuaStep::Redirect => {
                        if !Self::restart(session, ctx, &mut changes_left).await? {
                            return Ok(true);
                        }
                        continue 'request;
                    }
                    LuaStep::Go { .. } => {}
                }
            }

            let Some(mut path) = panel_routing::path::normalize(session.req_header().uri.path())
                .map(|path| path.into_owned())
            else {
                Self::error(
                    session,
                    ctx,
                    400,
                    "the request target is not an absolute path",
                    &[],
                )
                .await?;
                return Ok(true);
            };
            let request = Routed {
                header: session.req_header(),
                host: host_name,
                path: &path,
                client: ctx.client,
            };
            let found = match &named {
                Some(name) => routing.named(site_index, name),
                None => routing.select(site_index, &request),
            };
            let Some(mut route_index) = found else {
                match &named {
                    Some(name) => {
                        let message = format!("could not find named location \"@{name}\"");
                        Self::error(session, ctx, 500, &message, &[]).await?;
                    }
                    None => Self::error(session, ctx, 404, "not found", &[]).await?,
                }
                return Ok(true);
            };
            ctx.route = Some(route_index);
            loop {
                // nginx's internal locations answer requests from outside 404
                // once they are found, before their rewrites run.
                if site.route(route_index).internal && !ctx.internal && ctx.subrequest.is_none() {
                    Self::error(session, ctx, 404, "not found", &[]).await?;
                    return Ok(true);
                }
                if let LuaStep::Done = self
                    .lua_variables(
                        session,
                        ctx,
                        &site.route(route_index).lua.variables,
                        &path,
                        host_name,
                    )
                    .await?
                {
                    return Ok(true);
                }
                let rules = &site.route(route_index).rewrites;
                if !rules.is_empty() {
                    match self.rewrite(session, ctx, rules, &path, host_name).await? {
                        Rewrite::Answered => return Ok(true),
                        Rewrite::Unchanged => {}
                        Rewrite::Changed {
                            path: rewritten,
                            reroute: false,
                        } => path = rewritten,
                        Rewrite::Changed { reroute: true, .. } => {
                            let Some(next) = Self::choose_again(
                                session,
                                ctx,
                                routing,
                                site_index,
                                host_name,
                                &mut path,
                                &mut changes_left,
                            )
                            .await?
                            else {
                                return Ok(true);
                            };
                            route_index = next;
                            continue;
                        }
                    }
                }
                let Some(hook) = site.route(route_index).lua.rewrite.clone() else {
                    break;
                };
                let proxied = matches!(site.route(route_index).target, RouteTarget::Proxy(_));
                match self
                    .lua_request(session, ctx, &hook, &path, host_name, proxied)
                    .await?
                {
                    LuaStep::Done => return Ok(true),
                    LuaStep::Go { jump: false } => break,
                    LuaStep::Redirect => {
                        if !Self::restart(session, ctx, &mut changes_left).await? {
                            return Ok(true);
                        }
                        continue 'request;
                    }
                    LuaStep::Go { jump: true } => {
                        ctx.internal = true;
                        let Some(next) = Self::choose_again(
                            session,
                            ctx,
                            routing,
                            site_index,
                            host_name,
                            &mut path,
                            &mut changes_left,
                        )
                        .await?
                        else {
                            return Ok(true);
                        };
                        route_index = next;
                    }
                }
            }
            let route = site.route(route_index);
            self.lua_prepare_header_filter(session, ctx, &path, host_name);
            // access_by_lua_no_postpone: the access handler goes before the
            // security policies instead of after them.
            let access_first = Self::lua_plan(ctx).is_some_and(|plan| plan.access_first);
            // Subrequests skip the access phase, as nginx's do.
            let subrequest = ctx.subrequest.is_some();
            if !access_first
                && !subrequest
                && Self::admit(session, ctx, &snapshot, site, route, &path, host_name).await?
            {
                return Ok(true);
            }
            if let Some(hook) = route.lua.access.clone().filter(|_| !subrequest) {
                let proxied = matches!(route.target, RouteTarget::Proxy(_));
                match self
                    .lua_request(session, ctx, &hook, &path, host_name, proxied)
                    .await?
                {
                    LuaStep::Done => return Ok(true),
                    LuaStep::Redirect => {
                        if !Self::restart(session, ctx, &mut changes_left).await? {
                            return Ok(true);
                        }
                        continue 'request;
                    }
                    LuaStep::Go { .. } => {}
                }
            }
            if access_first
                && !subrequest
                && Self::admit(session, ctx, &snapshot, site, route, &path, host_name).await?
            {
                return Ok(true);
            }
            let http: Vec<&HttpPolicy> = site
                .http
                .into_iter()
                .chain(route.http)
                .map(|index| &snapshot.http[index])
                .collect();
            if !http.is_empty() {
                if let Some(answer) = http
                    .iter()
                    .rev()
                    .find_map(|policy| policy.cors.as_ref())
                    .and_then(|cors| cors.preflight(session.req_header()))
                {
                    let headers: Vec<(header::HeaderName, &str)> = answer
                        .iter()
                        .filter_map(|(name, value)| Some((name.clone(), value.to_str().ok()?)))
                        .collect();
                    responses::send(session, 204, &headers, bytes::Bytes::new()).await?;
                    return Ok(true);
                }
                let tls = self.listener.tls;
                let module = lua_phases::with_variables(session, &ctx.variables, |variables| {
                    let facts = facts(session, host_name, &path, tls, variables, &ctx.request_uri);
                    for policy in &http {
                        policy.request_changes(&facts, &mut ctx.http_request);
                    }
                    let mut module = HttpPolicyModule::default();
                    module.prepare(&http, &facts, session.req_header());
                    module
                });
                if let Some(slot) = session.downstream_modules_ctx.get_mut::<HttpPolicyModule>() {
                    *slot = module;
                }
                ctx.compression = site
                    .http
                    .into_iter()
                    .chain(route.http)
                    .rev()
                    .find(|index| snapshot.http[*index].compression.is_some())
                    .filter(|_| {
                        matches!(route.target, RouteTarget::Proxy(_) | RouteTarget::Static(_))
                    });
                if let Some(compression) = ctx
                    .compression
                    .and_then(|index| snapshot.http[index].compression.as_ref())
                {
                    compression.prepare(session);
                }
            }
            if let Some(hook) = route.lua.precontent.clone() {
                let proxied = matches!(route.target, RouteTarget::Proxy(_));
                match self
                    .lua_request(session, ctx, &hook, &path, host_name, proxied)
                    .await?
                {
                    LuaStep::Done => return Ok(true),
                    LuaStep::Redirect => {
                        if !Self::restart(session, ctx, &mut changes_left).await? {
                            return Ok(true);
                        }
                        continue 'request;
                    }
                    LuaStep::Go { .. } => {}
                }
            }
            return match &route.target {
                RouteTarget::Proxy(index) => {
                    // The upstream admits the request once the cache does not
                    // answer it, in proxy_upstream_filter.
                    ctx.upstream_target = Some(*index);
                    let cacheable = matches!(
                        session.req_header().method,
                        http::Method::GET | http::Method::HEAD
                    );
                    if let Some(plan) = route
                        .cache
                        .clone()
                        .filter(|_| cacheable && ctx.subrequest.is_none())
                    {
                        let request = Routed {
                            header: session.req_header(),
                            host: host_name,
                            path: &path,
                            client: ctx.client,
                        };
                        let primary = (!plan.bypasses(&request)).then(|| {
                            let tls = self.listener.tls;
                            lua_phases::with_variables(session, &ctx.variables, |variables| {
                                plan.key(&facts(
                                    session,
                                    host_name,
                                    &path,
                                    tls,
                                    variables,
                                    &ctx.request_uri,
                                ))
                            })
                        });
                        ctx.cache = Some(RequestCache {
                            plan,
                            site: site.id.to_string(),
                            primary,
                        });
                    }
                    Ok(false)
                }
                RouteTarget::Static(content) => {
                    let compression = ctx
                        .compression
                        .and_then(|index| snapshot.http[index].compression.as_ref());
                    if let Some(failure) = static_files::serve(
                        session,
                        &snapshot.statics[*content],
                        &path,
                        compression,
                    )
                    .await?
                    {
                        Self::error(
                            session,
                            ctx,
                            failure.status,
                            failure.message,
                            &failure.headers,
                        )
                        .await?;
                    }
                    Ok(true)
                }
                RouteTarget::Redirect {
                    location,
                    status,
                    preserve_path,
                } => {
                    let tls = self.listener.tls;
                    let rendered =
                        lua_phases::with_variables(session, &ctx.variables, |variables| {
                            location.render(&facts(
                                session,
                                host_name,
                                &path,
                                tls,
                                variables,
                                &ctx.request_uri,
                            ))
                        });
                    let location = String::from_utf8_lossy(&rendered).into_owned();
                    let location = if *preserve_path {
                        let target = session
                            .req_header()
                            .uri
                            .path_and_query()
                            .map_or("/", |value| value.as_str());
                        format!("{}{target}", location.trim_end_matches('/'))
                    } else {
                        location
                    };
                    responses::redirect(session, *status, &location).await?;
                    Ok(true)
                }
                RouteTarget::Respond {
                    status,
                    body,
                    content_type,
                    retry_after,
                } => {
                    let tls = self.listener.tls;
                    let body = lua_phases::with_variables(session, &ctx.variables, |variables| {
                        body.render(&facts(
                            session,
                            host_name,
                            &path,
                            tls,
                            variables,
                            &ctx.request_uri,
                        ))
                    });
                    let retry_after = retry_after.map(|seconds| seconds.to_string());
                    let mut headers = Vec::with_capacity(2);
                    if let Some(content_type) = content_type {
                        headers.push((header::CONTENT_TYPE, content_type.as_str()));
                    } else if !body.is_empty() {
                        headers.push((header::CONTENT_TYPE, "text/plain; charset=utf-8"));
                    }
                    if let Some(retry_after) = &retry_after {
                        headers.push((header::RETRY_AFTER, retry_after.as_str()));
                    }
                    // As nginx's error pages answer `return 503;`, a response
                    // without a body gets the page for its status.
                    if body.is_empty() && content_type.is_none() {
                        if let Some(answer) = match Self::page(session, ctx, *status) {
                            Some(prepared) => prepared.load().await,
                            None => None,
                        } {
                            let kept: Vec<_> = headers
                                .iter()
                                .filter(|(name, _)| *name == header::RETRY_AFTER)
                                .cloned()
                                .collect();
                            answer.send(session, &kept).await?;
                            return Ok(true);
                        }
                    }
                    responses::send(session, *status, &headers, body).await?;
                    Ok(true)
                }
                RouteTarget::Lua(hook) => {
                    let hook = hook.clone();
                    match self
                        .lua_request(session, ctx, &hook, &path, host_name, false)
                        .await?
                    {
                        LuaStep::Redirect => {
                            if !Self::restart(session, ctx, &mut changes_left).await? {
                                return Ok(true);
                            }
                            continue 'request;
                        }
                        _ => Ok(true),
                    }
                }
                RouteTarget::InternalRedirect(target) => {
                    match target {
                        InternalTarget::Named(name) => ctx.named = Some(name.clone()),
                        InternalTarget::Path(parts) => {
                            let tls = self.listener.tls;
                            let target =
                                lua_phases::with_variables(session, &ctx.variables, |variables| {
                                    InternalTarget::target(
                                        parts,
                                        &facts(
                                            session,
                                            host_name,
                                            &path,
                                            tls,
                                            variables,
                                            &ctx.request_uri,
                                        ),
                                    )
                                });
                            if !Self::retarget(session, ctx, target).await? {
                                return Ok(true);
                            }
                        }
                    }
                    if !Self::restart(session, ctx, &mut changes_left).await? {
                        return Ok(true);
                    }
                    continue 'request;
                }
            };
        }
    }

    /// Admits the request to its upstream once the cache did not answer it:
    /// a hit neither counts against the upstream's circuit nor waits for a
    /// place among the requests it takes at once.
    async fn proxy_upstream_filter(
        &self,
        session: &mut Session,
        ctx: &mut RequestContext,
    ) -> pingora_core::Result<bool> {
        let Some((snapshot, index)) = ctx.snapshot.clone().zip(ctx.upstream_target) else {
            return Ok(true);
        };
        if ctx.pool.is_some() {
            return Ok(true);
        }
        let pool = &snapshot.pools[index];
        match pool.admit() {
            Ok(trial) => ctx.trial = trial,
            Err(seconds) => {
                let wait = seconds.to_string();
                Self::error(
                    session,
                    ctx,
                    503,
                    "the upstream is failing; try again later",
                    &[(header::RETRY_AFTER, wait.as_str())],
                )
                .await?;
                return Ok(false);
            }
        }
        match pool.place().await {
            Ok(place) => ctx.place = place,
            Err(busy) => {
                if std::mem::take(&mut ctx.trial) {
                    pool.cancel_trial();
                }
                let message = match busy {
                    Busy::Full => "the upstream is handling all the requests it takes",
                    Busy::Waited => "the upstream did not take the request in time",
                };
                Self::error(session, ctx, 503, message, &[(header::RETRY_AFTER, "1")]).await?;
                return Ok(false);
            }
        }
        pool.count_request();
        ctx.pool = Some(index);
        Ok(true)
    }

    fn request_cache_filter(
        &self,
        session: &mut Session,
        ctx: &mut RequestContext,
    ) -> pingora_core::Result<()> {
        if let Some((snapshot, cache)) = ctx.snapshot.as_ref().zip(ctx.cache.as_ref()) {
            if cache.primary.is_some() {
                snapshot.cache.enable(session, &cache.plan);
            }
        }
        Ok(())
    }

    fn cache_key_callback(
        &self,
        _session: &Session,
        ctx: &mut RequestContext,
    ) -> pingora_core::Result<CacheKey> {
        let snapshot = ctx
            .snapshot
            .as_ref()
            .ok_or_else(|| Error::explain(ErrorType::InternalError, "request has no snapshot"))?;
        let (site, primary) = ctx
            .cache
            .as_ref()
            .and_then(|cache| Some((&cache.site, cache.primary.as_ref()?)))
            .ok_or_else(|| Error::explain(ErrorType::InternalError, "request has no cache key"))?;
        Ok(snapshot.cache.key(site, primary))
    }

    fn cache_vary_filter(
        &self,
        meta: &CacheMeta,
        ctx: &mut RequestContext,
        request: &RequestHeader,
    ) -> Option<HashBinary> {
        ctx.cache.as_ref()?.plan.variance(meta, request)
    }

    fn response_cache_filter(
        &self,
        session: &Session,
        response: &ResponseHeader,
        ctx: &mut RequestContext,
    ) -> pingora_core::Result<RespCacheable> {
        Ok(match &ctx.cache {
            Some(cache) => cache.plan.decide(
                response,
                session
                    .req_header()
                    .headers
                    .contains_key(header::AUTHORIZATION),
            ),
            None => RespCacheable::Uncacheable(NoCacheReason::NeverEnabled),
        })
    }

    async fn upstream_peer(
        &self,
        session: &mut Session,
        ctx: &mut RequestContext,
    ) -> pingora_core::Result<Box<HttpPeer>> {
        let snapshot = ctx
            .snapshot
            .clone()
            .ok_or_else(|| Error::explain(ErrorType::InternalError, "request has no snapshot"))?;
        let pool = &snapshot.pools[ctx
            .pool
            .ok_or_else(|| Error::explain(ErrorType::InternalError, "request has no pool"))?];
        let key = pool.hash.extract(
            session.req_header(),
            ctx.client
                .map(|client| SocketAddr::new(client, 0))
                .or_else(|| client_address(session)),
        );
        if !ctx.tried.is_empty() && ctx.retries > 0 {
            let delay = pool.retry.delay(ctx.retries);
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
        }
        if let Some(hook) = pool.balancer.clone() {
            if let Some((address, timeouts)) = self
                .lua_balancer(session, ctx, &hook, pool.id.as_str())
                .await?
            {
                let mut peer = pool.peer_at(address, is_upgrade_req(session.req_header()));
                if let Some(timeout) = timeouts.connect {
                    peer.options.connection_timeout = Some(timeout);
                }
                if let Some(timeout) = timeouts.read {
                    peer.options.read_timeout = Some(timeout);
                }
                if let Some(timeout) = timeouts.send {
                    peer.options.write_timeout = Some(timeout);
                }
                ctx.lua_peer = Some(address);
                ctx.endpoint = None;
                ctx.lease = None;
                ctx.failure_recorded = true;
                ctx.sent_at = Some(Instant::now());
                self.lua_upstream_certificate(session, ctx, &mut peer)
                    .await?;
                return Ok(Box::new(peer));
            }
        }
        let endpoint = pool.select(&key, &ctx.tried).ok_or_else(|| {
            Error::explain(
                ErrorType::HTTPStatus(503),
                format!("upstream {} has no available endpoint", pool.id),
            )
        })?;
        ctx.tried.push(endpoint);
        ctx.lease = Some(pool.lease(endpoint));
        ctx.endpoint = Some(endpoint);
        ctx.failure_recorded = false;
        ctx.sent_at = Some(Instant::now());
        let mut peer = pool.peer(endpoint, is_upgrade_req(session.req_header()));
        self.lua_upstream_certificate(session, ctx, &mut peer)
            .await?;
        Ok(Box::new(peer))
    }

    async fn upstream_request_filter(
        &self,
        session: &mut Session,
        upstream_request: &mut RequestHeader,
        ctx: &mut RequestContext,
    ) -> pingora_core::Result<()> {
        let Some((pool, _)) = ctx.upstream() else {
            return Ok(());
        };
        if !ctx.trusted_peer {
            for name in UNTRUSTED_FORWARDING {
                upstream_request.remove_header(name);
            }
        }
        if ctx.admission.strip_authorization {
            upstream_request.remove_header(&header::AUTHORIZATION);
        }
        forwarding::apply(
            upstream_request,
            &Forwarding {
                client: client_address(session).map(|address| address.ip()),
                tls: self.listener.tls,
                host_override: pool.host_header.as_deref(),
                close: !pool.keepalive && !is_upgrade_req(session.req_header()),
            },
        )?;
        if let Some(client) = ctx.client {
            upstream_request.insert_header(X_REAL_IP.clone(), client.to_string())?;
        }
        if let Some(body) = &ctx.lua_body {
            upstream_request.remove_header(&header::TRANSFER_ENCODING);
            upstream_request.insert_header(header::CONTENT_LENGTH, body.len().to_string())?;
        }
        http_policy::apply_to_request(upstream_request, &ctx.http_request)
    }

    async fn response_filter(
        &self,
        session: &mut Session,
        upstream_response: &mut ResponseHeader,
        ctx: &mut RequestContext,
    ) -> pingora_core::Result<()> {
        let snapshot = ctx.snapshot.clone();
        let status = upstream_response.status.as_u16();
        let head = session.req_header().method == http::Method::HEAD;
        let bodiless = upstream_response
            .headers
            .get(header::CONTENT_LENGTH)
            .is_some_and(|length| length.as_bytes() == b"0");
        // An upstream's error gets its page where the pages intercept it,
        // stored or not; a body that never comes could not carry the page,
        // so it is left be.
        if ctx.upstream_target.is_some() && status >= 400 && (head || !bodiless) {
            let intercepts = snapshot
                .as_deref()
                .zip(ctx.site)
                .is_some_and(|(snapshot, site)| {
                    let site = snapshot.routing.site(site);
                    ctx.route
                        .and_then(|route| site.route(route).error_pages.as_ref())
                        .unwrap_or(&site.error_pages)
                        .intercepts(status)
                });
            let answer = match Self::page(session, ctx, status).filter(|_| intercepts) {
                Some(prepared) => prepared.load().await,
                None => None,
            };
            if let Some(answer) = answer {
                let body = answer.replace(upstream_response)?;
                ctx.replacement = Some((!head).then_some(body));
            }
        }
        if let Some(compression) = snapshot
            .as_ref()
            .zip(ctx.compression)
            .and_then(|(snapshot, index)| snapshot.http[index].compression.as_ref())
        {
            compression.decide(session, upstream_response)?;
        }
        if let Some(cache) = &ctx.cache {
            if let Some(outcome) = Outcome::of(&session.cache.phase(), cache.primary.is_none()) {
                ctx.cache_outcome = Some(outcome);
                if let Some(snapshot) = &snapshot {
                    snapshot.cache.count(&cache.site, outcome);
                }
                // This cache is the last to handle the response, so its
                // entry goes after any the upstream sent (RFC 9211 §2).
                if cache.plan.policy.status_header {
                    upstream_response.append_header(
                        CACHE_STATUS,
                        format!("{CACHE_NAME}; {}", outcome.status()),
                    )?;
                }
            }
        }
        if upstream_response.status == http::StatusCode::SWITCHING_PROTOCOLS {
            // An upgraded connection idles as long as its protocol wants.
            session.set_read_timeout(None);
        }
        let bodied = !matches!(upstream_response.status.as_u16(), 100..=199 | 204 | 304)
            && session.req_header().method != http::Method::HEAD;
        if bodied && Self::lua_hook(ctx, |hooks| &hooks.body_filter).is_some() {
            // The filter may change the body's length.
            upstream_response.remove_header(&header::CONTENT_LENGTH);
            if !session.is_http2() && session.req_header().version == http::Version::HTTP_11 {
                upstream_response.insert_header(header::TRANSFER_ENCODING, "chunked")?;
            }
        }
        Ok(())
    }

    async fn response_body_filter(
        &self,
        session: &mut Session,
        body: &mut Option<bytes::Bytes>,
        end_of_stream: bool,
        ctx: &mut RequestContext,
    ) -> pingora_core::Result<Option<std::time::Duration>> {
        if let Some(replacement) = ctx.replacement.as_mut() {
            *body = replacement.take();
        }
        if let Some(hook) = Self::lua_hook(ctx, |hooks| &hooks.body_filter) {
            self.lua_body_filter(session, ctx, &hook, body, end_of_stream)
                .await?;
        }
        Ok(None)
    }

    async fn request_body_filter(
        &self,
        session: &mut Session,
        body: &mut Option<bytes::Bytes>,
        _end_of_stream: bool,
        ctx: &mut RequestContext,
    ) -> pingora_core::Result<()> {
        if session.was_upgraded() {
            return Ok(());
        }
        if ctx.lua_body.is_some() {
            *body = ctx.lua_body.take();
            return Ok(());
        }
        if let (Some(limit), Some(chunk)) = (ctx.admission.max_body_bytes, body.as_ref()) {
            ctx.body_seen += u64::try_from(chunk.len()).unwrap_or(u64::MAX);
            if ctx.body_seen > limit {
                return Error::e_explain(
                    ErrorType::HTTPStatus(413),
                    "the request body is larger than its security policy allows",
                );
            }
        }
        Ok(())
    }

    async fn upstream_response_filter(
        &self,
        session: &mut Session,
        upstream_response: &mut ResponseHeader,
        ctx: &mut RequestContext,
    ) -> pingora_core::Result<()> {
        if upstream_response.status.is_informational() {
            return Ok(());
        }
        let sent_at = ctx.sent_at.take();
        if let (Some((pool, endpoint)), Some(sent_at)) = (ctx.upstream(), sent_at) {
            pool.record_latency(endpoint, sent_at.elapsed());
        }
        let status = upstream_response.status.as_u16();
        self.measure_upstream(
            &session.req_header().method,
            ctx,
            sent_at,
            Some(status),
            None,
        );
        let retried = ctx.upstream().is_some_and(|(pool, _)| {
            pool.retry.retries_status(status)
                && RequestContext::replayable(session)
                && pool.may_retry(ctx.retries)
        });
        if retried {
            ctx.record_outcome(matches!(status, 502..=504));
            ctx.retries += 1;
            ctx.retrying = true;
            let mut error = Error::explain(
                ErrorType::HTTPStatus(status),
                "the upstream's status is retried",
            );
            error.esource = ErrorSource::Upstream;
            error.set_retry(true);
            return Err(error);
        }
        Ok(())
    }

    async fn connected_to_upstream(
        &self,
        session: &mut Session,
        reused: bool,
        peer: &HttpPeer,
        #[cfg(unix)] _fd: std::os::unix::io::RawFd,
        #[cfg(windows)] _sock: std::os::windows::io::RawSocket,
        digest: Option<&pingora_core::protocols::Digest>,
        ctx: &mut RequestContext,
    ) -> pingora_core::Result<()> {
        // A connection is judged once, as its handshake is, not each time
        // it is reused.
        if !reused && peer.is_tls() {
            self.lua_upstream_verify(session, ctx, peer, digest).await?;
        }
        let upstream = ctx
            .snapshot
            .as_ref()
            .zip(ctx.pool.zip(ctx.endpoint))
            .and_then(|(snapshot, (pool, endpoint))| snapshot.labels.endpoint(pool, endpoint));
        if let (Some(metrics), Some(endpoint)) = (&self.listener.metrics, upstream) {
            metrics.upstream_connection(endpoint.upstream, reused);
        }
        Ok(())
    }

    fn fail_to_connect(
        &self,
        session: &mut Session,
        _peer: &HttpPeer,
        ctx: &mut RequestContext,
        mut error: Box<Error>,
    ) -> Box<Error> {
        let sent_at = ctx.sent_at.take();
        self.measure_upstream(
            &session.req_header().method,
            ctx,
            sent_at,
            None,
            Some(&error),
        );
        ctx.record_outcome(true);
        if ctx.lua_peer.take().is_some() {
            ctx.lua_last_failure = Some(("failed".into(), None));
            if ctx.lua_more_tries > 0 {
                ctx.lua_more_tries -= 1;
                error.set_retry(true);
                return error;
            }
        }
        // Nothing reached the upstream, so trying another endpoint is safe.
        let retried = ctx
            .upstream()
            .is_some_and(|(pool, _)| pool.may_fail_over(ctx.tried.len(), ctx.retries));
        if retried {
            if ctx
                .upstream()
                .is_some_and(|(pool, _)| pool.retry.attempts > 0)
            {
                ctx.retries += 1;
            }
            error.set_retry(true);
        }
        error
    }

    fn error_while_proxy(
        &self,
        peer: &HttpPeer,
        session: &mut Session,
        error: Box<Error>,
        ctx: &mut RequestContext,
        client_reused: bool,
    ) -> Box<Error> {
        let retrying = std::mem::take(&mut ctx.retrying);
        if !retrying {
            let sent_at = ctx.sent_at.take();
            self.measure_upstream(
                &session.req_header().method,
                ctx,
                sent_at,
                None,
                Some(&error),
            );
        }
        if !ctx.failure_recorded && error.esource() == &pingora_core::ErrorSource::Upstream {
            ctx.record_outcome(true);
        }
        let mut error = error.more_context(format!("Peer: {peer}"));
        if !RequestContext::replayable(session) {
            error.set_retry(false);
        } else if retrying {
            error.set_retry(true);
        } else if ctx.upstream().is_some_and(|(pool, _)| {
            pool.retry.retries_error(error.etype()) && pool.may_retry(ctx.retries)
        }) {
            ctx.retries += 1;
            error.set_retry(true);
        } else {
            error.retry.decide_reuse(client_reused);
        }
        error
    }

    /// Pingora's answers to failed requests, except that a request body
    /// that stops arriving gets 408 (RFC 9110 §15.5.9) instead of 400.
    async fn fail_to_proxy(
        &self,
        session: &mut Session,
        error: &Error,
        ctx: &mut RequestContext,
    ) -> FailToProxy {
        http_policy::disable_compression(session);
        let code = match (error.etype(), error.esource()) {
            // A script closed the connection without an answer, or the
            // client closed it while a script watched.
            (ErrorType::HTTPStatus(444 | 499), _) => 0,
            (ErrorType::HTTPStatus(code), _) => *code,
            (ErrorType::ReadTimedout, ErrorSource::Downstream) => 408,
            (_, ErrorSource::Upstream) => 502,
            (
                ErrorType::WriteError | ErrorType::ReadError | ErrorType::ConnectionClosed,
                ErrorSource::Downstream,
            ) => 0,
            (_, ErrorSource::Downstream) => 400,
            _ => 500,
        };
        let page = match Self::page(session, ctx, code)
            .filter(|_| code > 0 && session.response_written().is_none())
        {
            Some(prepared) => prepared.load().await,
            None => None,
        };
        let sent = match page {
            Some(answer) => answer.send(session, &[]).await,
            None if code > 0 => session.respond_error(code).await,
            None => Ok(()),
        };
        if let Err(failure) = sent {
            tracing::debug!(%failure, "the error response did not reach the client");
        }
        FailToProxy {
            error_code: code,
            can_reuse_downstream: false,
        }
    }

    async fn logging(
        &self,
        session: &mut Session,
        error: Option<&Error>,
        ctx: &mut RequestContext,
    ) {
        if !session.is_http2() {
            if let Some(socket) = session
                .digest()
                .and_then(|digest| digest.socket_digest.as_ref())
            {
                self.listener.connections.request_done(socket);
            }
        }
        let status = session.response_written().map_or_else(
            || match error.map(|error| error.etype()) {
                Some(ErrorType::HTTPStatus(499)) => 499,
                _ => 0,
            },
            |response| response.status.as_u16(),
        );
        if ctx.upstream().is_some() && !ctx.failure_recorded {
            ctx.record_outcome(
                error.is_some_and(|error| error.esource() == &pingora_core::ErrorSource::Upstream)
                    || matches!(status, 502..=504),
            );
        }
        if std::mem::take(&mut ctx.trial) {
            if let Some(pool) = ctx
                .snapshot
                .as_ref()
                .zip(ctx.pool)
                .map(|(s, i)| &s.pools[i])
            {
                pool.cancel_trial();
            }
        }
        ctx.lease = None;
        // A subrequest is not logged; it gives its variables back.
        if let Some(mut subrequest) = ctx.subrequest.take() {
            let variables = lua_phases::with_variables(session, &ctx.variables, Clone::clone);
            subrequest.finish(variables);
            return;
        }
        if let Some(hook) = Self::lua_hook(ctx, |hooks| &hooks.log) {
            self.lua_log_phase(session, ctx, &hook, status).await;
        }
        self.measure(session, error, ctx, status);
        self.record(session, error, ctx, status);
        if tracing::enabled!(tracing::Level::DEBUG) {
            let site = ctx
                .snapshot
                .as_ref()
                .zip(ctx.site)
                .map(|(snapshot, index)| snapshot.routing.site(index));
            let route = site.zip(ctx.route).map(|(site, index)| site.route(index));
            tracing::debug!(
                event = "request",
                listener = %self.listener.id,
                site = site.map(|site| site.id.as_str()),
                route = route.map(|route| route.id.as_str()),
                route_name = route.and_then(|route| route.name.as_deref()),
                status,
                error = error.map(|error| error.to_string()),
            );
        }
    }
}

impl PanelProxy {
    /// Checks the request against the security policies of its site and
    /// route. Returns whether one refused it, answering it.
    async fn admit(
        session: &mut Session,
        ctx: &mut RequestContext,
        snapshot: &PreparedPingoraSnapshot,
        site: &SiteRoutes,
        route: &CompiledRoute,
        path: &str,
        host_name: &str,
    ) -> pingora_core::Result<bool> {
        let gates: Vec<usize> = site.security.into_iter().chain(route.security).collect();
        if gates.is_empty() {
            return Ok(false);
        }
        let mut admission = Admission::default();
        let candidate = Candidate {
            client: ctx.client.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
            header: session.req_header(),
            path,
            host: host_name,
            route: route.id.as_str(),
        };
        let mut refused = None;
        for index in gates {
            if let Err(refusal) = snapshot.policies[index]
                .check(&candidate, &mut admission)
                .await
            {
                refused = Some(refusal);
                break;
            }
        }
        if let Some(refusal) = refused {
            Self::refuse(session, ctx, refusal).await?;
            return Ok(true);
        }
        if let Some(timeout) = admission.body_timeout {
            session.set_read_timeout(Some(timeout));
        }
        ctx.admission = admission;
        Ok(false)
    }

    /// Forgets what the route a request took prepared, before the request
    /// starts over after `ngx.exec`; answers 500 instead once the request
    /// has changed its URI too often. Returns whether it starts over.
    async fn restart(
        session: &mut Session,
        ctx: &mut RequestContext,
        changes_left: &mut usize,
    ) -> pingora_core::Result<bool> {
        if *changes_left == 0 {
            Self::cycle(session, ctx).await?;
            return Ok(false);
        }
        *changes_left -= 1;
        ctx.internal = true;
        ctx.route = None;
        ctx.admission = Admission::default();
        ctx.http_request.clear();
        if ctx.compression.take().is_some() {
            http_policy::disable_compression(session);
        }
        if let Some(module) = session.downstream_modules_ctx.get_mut::<HttpPolicyModule>() {
            *module = HttpPolicyModule::default();
        }
        Ok(true)
    }

    /// Runs a site's or route's rewrite rules on the request (ADR 0040).
    async fn rewrite(
        &self,
        session: &mut Session,
        ctx: &mut RequestContext,
        rules: &Rewrites,
        path: &str,
        host: &str,
    ) -> pingora_core::Result<Rewrite> {
        let tls = self.listener.tls;
        let outcome = lua_phases::with_variables(session, &ctx.variables, |variables| {
            let facts = facts(session, host, path, tls, variables, &ctx.request_uri);
            rules.apply(path, facts.query, &facts)
        });
        let (path, query, reroute) = match outcome {
            Ok(Rewritten::Redirect { status, location }) => {
                responses::redirect(session, status, &location).await?;
                return Ok(Rewrite::Answered);
            }
            Ok(Rewritten::Path { changed: false, .. }) => return Ok(Rewrite::Unchanged),
            Ok(Rewritten::Path {
                path,
                query,
                reroute,
                ..
            }) => (path, query, reroute),
            Err(message) => {
                Self::retarget(session, ctx, Err(message)).await?;
                return Ok(Rewrite::Answered);
            }
        };
        if !Self::retarget(session, ctx, Ok((path.clone(), query))).await? {
            return Ok(Rewrite::Answered);
        }
        ctx.internal = true;
        Ok(Rewrite::Changed { path, reroute })
    }

    /// Gives the request `target`, a path and query; whether it could,
    /// answering 500 when it could not.
    async fn retarget(
        session: &mut Session,
        ctx: &RequestContext,
        target: Result<(String, Option<String>), String>,
    ) -> pingora_core::Result<bool> {
        let uri = target.and_then(|(path, query)| {
            let target = match query {
                Some(query) => format!("{path}?{query}"),
                None => path,
            };
            target
                .parse::<http::Uri>()
                .map_err(|error| format!("the rewritten target {target:?} is invalid: {error}"))
        });
        match uri {
            Ok(uri) => {
                session.req_header_mut().set_uri(uri);
                Ok(true)
            }
            Err(message) => {
                tracing::warn!(event = "rewrite_failed", message = %message);
                Self::error(session, ctx, 500, "the request could not be rewritten", &[]).await?;
                Ok(false)
            }
        }
    }

    /// Chooses the route of the site at `site` again for the request's
    /// path, one of the URI changes the request may make; `None` once the
    /// request is answered.
    async fn choose_again(
        session: &mut Session,
        ctx: &mut RequestContext,
        routing: &RoutingTable,
        site: usize,
        host: &str,
        path: &mut String,
        changes_left: &mut usize,
    ) -> pingora_core::Result<Option<usize>> {
        if *changes_left == 0 {
            Self::cycle(session, ctx).await?;
            return Ok(None);
        }
        *changes_left -= 1;
        let Some(rewritten) = panel_routing::path::normalize(session.req_header().uri.path())
            .map(|path| path.into_owned())
        else {
            Self::error(
                session,
                ctx,
                400,
                "the rewritten target is not an absolute path",
                &[],
            )
            .await?;
            return Ok(None);
        };
        *path = rewritten;
        let request = Routed {
            header: session.req_header(),
            host,
            path,
            client: ctx.client,
        };
        let Some(next) = routing.select(site, &request) else {
            Self::error(session, ctx, 404, "not found", &[]).await?;
            return Ok(None);
        };
        ctx.route = Some(next);
        Ok(Some(next))
    }

    /// The page answering the request's errors with `status`: its route's,
    /// or else its site's.
    fn page(session: &Session, ctx: &RequestContext, status: u16) -> Option<Prepared> {
        let snapshot = ctx.snapshot.as_deref()?;
        let site = snapshot.routing.site(ctx.site?);
        let pages = ctx
            .route
            .and_then(|route| site.route(route).error_pages.as_ref())
            .unwrap_or(&site.error_pages);
        let page = pages.page(status)?;
        let path = panel_routing::path::normalize(session.req_header().uri.path()).map_or_else(
            || session.req_header().uri.path().to_owned(),
            |path| path.into_owned(),
        );
        Some(lua_phases::with_variables(
            session,
            &ctx.variables,
            |variables| {
                page.prepare(
                    status,
                    &facts(
                        session,
                        &ctx.host,
                        &path,
                        ctx.scheme == "https",
                        variables,
                        &ctx.request_uri,
                    ),
                )
            },
        ))
    }

    /// Answers an error the gateway makes: with the request's page for
    /// `status`, keeping `headers`, or with `message` (ADR 0041).
    async fn error(
        session: &mut Session,
        ctx: &RequestContext,
        status: u16,
        message: &str,
        headers: &[(header::HeaderName, &str)],
    ) -> pingora_core::Result<()> {
        if let Some(prepared) = Self::page(session, ctx, status) {
            if let Some(answer) = prepared.load().await {
                return answer.send(session, headers).await;
            }
        }
        responses::plain(session, status, message, headers).await
    }

    /// Answers a request that changed its URI more often than it may, as
    /// nginx does.
    async fn cycle(session: &mut Session, ctx: &RequestContext) -> pingora_core::Result<()> {
        Self::error(
            session,
            ctx,
            500,
            "rewrite or internal redirection cycle",
            &[],
        )
        .await
    }

    /// Answers a request a security policy refused, with the page for its
    /// status when the policy gives no body of its own.
    async fn refuse(
        session: &mut Session,
        ctx: &RequestContext,
        refusal: Refusal,
    ) -> pingora_core::Result<()> {
        if refusal.custom.is_none() {
            let headers: Vec<(header::HeaderName, &str)> = refusal
                .headers
                .iter()
                .map(|(name, value)| (name.clone(), value.as_str()))
                .collect();
            if let Some(answer) = match Self::page(session, ctx, refusal.status) {
                Some(prepared) => prepared.load().await,
                None => None,
            } {
                return answer.send(session, &headers).await;
            }
        }
        refuse(session, refusal).await
    }

    fn scheme(&self) -> &'static str {
        if self.listener.tls {
            "https"
        } else {
            "http"
        }
    }

    /// Writes the access record of a request that is done, and an error
    /// record when it failed (ADR 0025).
    fn record(&self, session: &Session, error: Option<&Error>, ctx: &RequestContext, status: u16) {
        let Some(logs) = &self.listener.logs else {
            return;
        };
        let snapshot = ctx.snapshot.as_deref();
        let (logging, plan) = match snapshot {
            Some(snapshot) => (
                &snapshot.logging,
                snapshot.routing.access(ctx.site, ctx.route),
            ),
            None => (LoggingPlan::fallback(), AccessPlan::fallback()),
        };
        if !plan.enabled && error.is_none() {
            return;
        }
        let request = session.req_header();
        let labels = snapshot.map(|snapshot| &snapshot.labels);
        let site = labels
            .zip(ctx.site)
            .and_then(|(labels, site)| labels.site(site));
        let route = labels
            .zip(ctx.site.zip(ctx.route))
            .and_then(|(labels, (site, route))| labels.route(site, route));
        let endpoint = labels
            .zip(ctx.pool.zip(ctx.endpoint))
            .and_then(|(labels, (pool, endpoint))| labels.endpoint(pool, endpoint));
        let node = endpoint
            .as_ref()
            .map(|endpoint| format!("{}:{}", endpoint.address, endpoint.port));
        let host = hosts::request_host(request).ok().flatten();
        let status = (status > 0).then_some(status);
        lua_phases::with_variables(session, &ctx.variables, |variables| {
            let served = Served {
                variables,
                request,
                scheme: self.scheme(),
                host: host.as_ref().map(|host| host.name.as_str()),
                client: ctx.client,
                peer: client_address(session),
                status,
                request_bytes: u64::try_from(session.body_bytes_read()).unwrap_or(u64::MAX),
                response_bytes: u64::try_from(session.body_bytes_sent()).unwrap_or(u64::MAX),
                duration: ctx.started.elapsed(),
                error_type: telemetry::server_error_type(error, status),
                listener: &self.listener.id,
                site: site.as_deref(),
                route: route.as_deref(),
                upstream: endpoint.as_ref().map(|endpoint| &*endpoint.upstream),
                node: node.as_deref(),
                original: (!ctx.request_uri.is_empty()
                    && request
                        .uri
                        .path_and_query()
                        .is_none_or(|target| target.as_str() != ctx.request_uri))
                .then_some(ctx.request_uri.as_str()),
                cache: ctx.cache_outcome,
            };
            let now = Utc::now();
            if plan.enabled {
                let line = match plan.format {
                    AccessLogFormat::Json => access_log::json(&served, plan, logging, now),
                    AccessLogFormat::Combined => access_log::combined(&served, logging, now),
                };
                let destination = site.clone().map_or(Destination::Gateway, Destination::Site);
                logs.send(destination, line, logging.files);
            }
            if let Some(error) = error {
                logs.send(
                    Destination::Errors,
                    access_log::error(&served, error, logging, now),
                    logging.files,
                );
            }
        });
    }

    /// Records a request that is done, and stops counting it as active.
    fn measure(
        &self,
        session: &Session,
        error: Option<&Error>,
        ctx: &mut RequestContext,
        status: u16,
    ) {
        let active = ctx.active.take();
        let Some(metrics) = &self.listener.metrics else {
            return;
        };
        let request = session.req_header();
        let status = (status > 0).then_some(status);
        let labels = ctx.snapshot.as_ref().map(|snapshot| &snapshot.labels);
        let site = labels
            .zip(ctx.site)
            .and_then(|(labels, site)| labels.site(site));
        let route = labels
            .zip(ctx.site.zip(ctx.route))
            .and_then(|(labels, (site, route))| labels.route(site, route));
        let measured = ServerRequest {
            http_request_method: method(&request.method),
            url_scheme: self.scheme(),
            http_response_status_code: status,
            network_protocol_version: protocol_version(request.version),
            error_type: telemetry::server_error_type(error, status),
            site,
            route,
        };
        metrics.server.finish(
            &measured,
            ctx.started.elapsed(),
            u64::try_from(session.body_bytes_read()).unwrap_or(u64::MAX),
            u64::try_from(session.body_bytes_sent()).unwrap_or(u64::MAX),
        );
        let domain = labels
            .zip(ctx.domain)
            .and_then(|(labels, domain)| labels.domain(domain));
        if let Some((site, domain)) = measured.site.zip(domain) {
            metrics.domain_request(site, domain);
        }
        drop(active);
    }

    /// Records an attempt sent upstream at `sent_at`, once it has a response
    /// or failed.
    fn measure_upstream(
        &self,
        request_method: &http::Method,
        ctx: &RequestContext,
        sent_at: Option<Instant>,
        status: Option<u16>,
        error: Option<&Error>,
    ) {
        let (Some(metrics), Some(sent_at)) = (&self.listener.metrics, sent_at) else {
            return;
        };
        let Some(endpoint) = ctx
            .snapshot
            .as_ref()
            .zip(ctx.pool.zip(ctx.endpoint))
            .and_then(|(snapshot, (pool, endpoint))| snapshot.labels.endpoint(pool, endpoint))
        else {
            return;
        };
        metrics.client.finish(
            &ClientRequest {
                http_request_method: method(request_method),
                server_address: endpoint.address,
                server_port: endpoint.port,
                http_response_status_code: status,
                error_type: telemetry::client_error_type(error, status),
                upstream: endpoint.upstream,
            },
            sent_at.elapsed(),
        );
    }

    /// The key authorization for an HTTP-01 challenge this request fetches.
    async fn challenge_answer(&self, session: &Session) -> Option<bytes::Bytes> {
        let challenges = self.listener.challenges.as_ref()?;
        let request = session.req_header();
        if request.method != http::Method::GET && request.method != http::Method::HEAD {
            return None;
        }
        challenges.answer(request.uri.path()).await
    }

    /// Enforces the presented certificate's minimum TLS version and rejects
    /// requests for hosts the connection's certificate does not cover.
    fn tls_rejection(
        &self,
        session: &Session,
        snapshot: &PreparedPingoraSnapshot,
        host: &str,
    ) -> Option<(u16, &'static str)> {
        if !self.listener.tls {
            return None;
        }
        let digest = session.digest()?.ssl_digest.as_ref()?;
        let handshake = digest.extension.get::<Handshake>()?;
        let certificates = snapshot.certificates.load();
        let presented = certificates.presented(&self.listener.id, handshake.server_name.as_deref());
        if let (Some(certificate), Some(version)) = (presented, handshake.version) {
            if version < certificate.minimum {
                return Some((403, "this site requires a newer TLS version"));
            }
        }
        match &handshake.server_name {
            Some(server_name)
                if !host.is_empty()
                    && server_name != host
                    && !certificates.covers(&self.listener.id, server_name, host) =>
            {
                Some((421, "the connection's certificate does not cover this host"))
            }
            _ => None,
        }
    }
}

/// What a template may name about the request being answered.
fn facts<'a>(
    session: &'a Session,
    host: &'a str,
    path: &'a str,
    tls: bool,
    variables: &'a HashMap<String, String>,
    request_uri: &'a str,
) -> Facts<'a> {
    Facts {
        host,
        uri: path,
        query: session.req_header().uri.query(),
        request_uri,
        method: session.req_header().method.as_str(),
        scheme: if tls { "https" } else { "http" },
        client_ip: client_address(session).map(|address| address.ip()),
        headers: &session.req_header().headers,
        upstream: None,
        cache_status: None,
        variables,
    }
}

fn client_address(session: &Session) -> Option<SocketAddr> {
    session
        .client_addr()
        .and_then(|address| address.as_inet())
        .copied()
}

/// One redirect covering alias, `www` and HTTPS rules, so clients never follow a chain.
fn site_redirect(
    site: &SiteRoutes,
    host: Option<&RequestHost>,
    alias: bool,
    tls: bool,
    request: &RequestHeader,
) -> Option<String> {
    let host = host?;
    if host.name.starts_with('[') || host.name.parse::<IpAddr>().is_ok() {
        return None;
    }
    let mut name = host.name.as_str();
    let mut changed = false;
    if alias {
        if let Some(primary) = site.primary.as_deref().filter(|primary| *primary != name) {
            name = primary;
            changed = true;
        }
    }
    if let Some(target) = site.www_target(name) {
        name = target;
        changed = true;
    }
    let (scheme, port) = if !tls && site.https_redirect {
        changed = true;
        ("https", site.https_port)
    } else {
        let default = if tls { 443 } else { 80 };
        (
            if tls { "https" } else { "http" },
            host.port.filter(|port| *port != default),
        )
    };
    if !changed {
        return None;
    }
    let target = request
        .uri
        .path_and_query()
        .map_or("/", |value| value.as_str());
    Some(match port {
        Some(port) => format!("{scheme}://{name}:{port}{target}"),
        None => format!("{scheme}://{name}{target}"),
    })
}
