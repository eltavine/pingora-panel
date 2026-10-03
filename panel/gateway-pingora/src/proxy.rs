//! The request path: host and site resolution, redirects, route actions and
//! upstream selection against the active snapshot.

use crate::{
    acme::ChallengeDirectory,
    adapter::{ActiveSnapshot, PreparedPingoraSnapshot},
    certificates::Handshake,
    forwarding::{self, Forwarding},
    hosts::{self, HostError, RequestHost},
    hsts::{StrictTransport, StrictTransportBuilder},
    path, responses,
    routing::{RouteTarget, SiteRoutes},
    static_files,
    template::Facts,
    upstream::{EndpointLease, UpstreamPool},
};
use async_trait::async_trait;
use http::header;
use pingora_core::{
    modules::http::{compression::ResponseCompressionBuilder, HttpModules},
    upstreams::peer::HttpPeer,
    Error, ErrorType,
};
use pingora_http::{RequestHeader, ResponseHeader};
use pingora_proxy::{ProxyHttp, Session};
use std::{
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicUsize, Ordering::Relaxed},
        Arc,
    },
    time::Instant,
};

const REDIRECT_STATUS: u16 = 308;

/// What a listener contributes to each request it accepts.
pub(crate) struct ListenerContext {
    pub id: String,
    pub tls: bool,
    pub http1: bool,
    pub challenges: Option<Arc<ChallengeDirectory>>,
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
    route: Option<usize>,
    pool: Option<usize>,
    tried: Vec<usize>,
    endpoint: Option<usize>,
    lease: Option<EndpointLease>,
    sent_at: Option<Instant>,
    failure_recorded: bool,
}

impl RequestContext {
    fn upstream(&self) -> Option<(&UpstreamPool, usize)> {
        let pool = &self.snapshot.as_ref()?.pools[self.pool?];
        Some((pool, self.endpoint?))
    }
}

#[async_trait]
impl ProxyHttp for PanelProxy {
    type CTX = RequestContext;

    fn init_downstream_modules(&self, modules: &mut HttpModules) {
        modules.add_module(ResponseCompressionBuilder::enable(0));
        modules.add_module(Box::new(StrictTransportBuilder));
    }

    fn new_ctx(&self) -> RequestContext {
        self.in_flight.fetch_add(1, Relaxed);
        RequestContext {
            _in_flight: InFlight(Arc::clone(&self.in_flight)),
            snapshot: None,
            site: None,
            route: None,
            pool: None,
            tried: Vec::new(),
            endpoint: None,
            lease: None,
            sent_at: None,
            failure_recorded: false,
        }
    }

    async fn request_filter(
        &self,
        session: &mut Session,
        ctx: &mut RequestContext,
    ) -> pingora_core::Result<bool> {
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
        if let Some(rejection) = self.tls_rejection(session, &snapshot, host_name) {
            let (status, message) = rejection;
            responses::plain(session, status, message, &[]).await?;
            return Ok(true);
        }

        let routing = &snapshot.routing;
        let entry = routing
            .lookup(host_name)
            .filter(|entry| routing.site(entry.site).serves(&self.listener.id));
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

        let Some(path) =
            path::normalize(session.req_header().uri.path()).map(|path| path.into_owned())
        else {
            responses::plain(
                session,
                400,
                "the request target is not an absolute path",
                &[],
            )
            .await?;
            return Ok(true);
        };
        let Some(route_index) = site.select(host_name, &path) else {
            responses::plain(session, 404, "not found", &[]).await?;
            return Ok(true);
        };
        ctx.route = Some(route_index);
        match &site.route(route_index).target {
            RouteTarget::Proxy(pool) => {
                ctx.pool = Some(*pool);
                Ok(false)
            }
            RouteTarget::Static(content) => {
                static_files::serve(session, &snapshot.statics[*content], &path).await?;
                Ok(true)
            }
            RouteTarget::Redirect {
                location,
                status,
                preserve_path,
            } => {
                let rendered =
                    location.render(&facts(session, host_name, &path, self.listener.tls));
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
                let body = body.render(&facts(session, host_name, &path, self.listener.tls));
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
                responses::send(session, *status, &headers, body).await?;
                Ok(true)
            }
        }
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
        let key = pool
            .hash
            .extract(session.req_header(), client_address(session));
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
        Ok(Box::new(pool.peer(endpoint)))
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
        forwarding::apply(
            upstream_request,
            &Forwarding {
                client: client_address(session).map(|address| address.ip()),
                tls: self.listener.tls,
                host_override: pool.host_header.as_deref(),
                close: !pool.keepalive,
            },
        )
    }

    async fn upstream_response_filter(
        &self,
        _session: &mut Session,
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
        Ok(())
    }

    fn fail_to_connect(
        &self,
        _session: &mut Session,
        _peer: &HttpPeer,
        ctx: &mut RequestContext,
        mut error: Box<Error>,
    ) -> Box<Error> {
        if let Some((pool, endpoint)) = ctx.upstream() {
            pool.record_failure(endpoint);
            // Nothing reached the upstream, so trying another endpoint is safe.
            if pool.may_fail_over(ctx.tried.len()) {
                error.set_retry(true);
            }
        }
        ctx.failure_recorded = true;
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
        if !ctx.failure_recorded && error.esource() == &pingora_core::ErrorSource::Upstream {
            if let Some((pool, endpoint)) = ctx.upstream() {
                pool.record_failure(endpoint);
            }
            ctx.failure_recorded = true;
        }
        let mut error = error.more_context(format!("Peer: {peer}"));
        if !session.req_header().method.is_idempotent() || session.as_ref().retry_buffer_truncated()
        {
            error.set_retry(false);
        } else {
            error.retry.decide_reuse(client_reused);
        }
        error
    }

    async fn logging(
        &self,
        session: &mut Session,
        error: Option<&Error>,
        ctx: &mut RequestContext,
    ) {
        let status = session
            .response_written()
            .map_or(0, |response| response.status.as_u16());
        if let Some((pool, endpoint)) = ctx.upstream() {
            if !ctx.failure_recorded {
                if error
                    .is_some_and(|error| error.esource() == &pingora_core::ErrorSource::Upstream)
                    || matches!(status, 502..=504)
                {
                    pool.record_failure(endpoint);
                } else {
                    pool.record_success(endpoint);
                }
            }
        }
        ctx.lease = None;
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
fn facts<'a>(session: &'a Session, host: &'a str, path: &'a str, tls: bool) -> Facts<'a> {
    Facts {
        host,
        uri: path,
        method: session.req_header().method.as_str(),
        scheme: if tls { "https" } else { "http" },
        client_ip: client_address(session).map(|address| address.ip()),
        headers: &session.req_header().headers,
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
