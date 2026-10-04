//! The request path: host and site resolution, redirects, route actions and
//! upstream selection against the active snapshot.

use crate::{
    access_log::{self, AccessPlan, LoggingPlan, Served},
    acme::ChallengeDirectory,
    adapter::{ActiveSnapshot, PreparedPingoraSnapshot},
    certificates::Handshake,
    forwarding::{self, Forwarding},
    head_deadline::Connections,
    hosts::{self, HostError, RequestHost},
    hsts::{StrictTransport, StrictTransportBuilder},
    log_files::{Destination, Logs},
    path, request_identity, responses,
    routing::{RouteTarget, SiteRoutes},
    security::{Admission, Candidate, ClientResolution, Refusal},
    static_files,
    telemetry::{self, GatewayMetrics},
    template::Facts,
    upstream::{EndpointLease, UpstreamPool},
};
use async_trait::async_trait;
use chrono::Utc;
use http::header;
use panel_ir::AccessLogFormat;
use panel_metrics::{method, protocol_version, ActiveRequest, ClientRequest, ServerRequest};
use pingora_core::{
    modules::http::{compression::ResponseCompressionBuilder, HttpModules},
    upstreams::peer::HttpPeer,
    Error, ErrorSource, ErrorType,
};
use pingora_http::{RequestHeader, ResponseHeader};
use pingora_proxy::{FailToProxy, ProxyHttp, Session};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        atomic::{AtomicUsize, Ordering::Relaxed},
        Arc,
    },
    time::Instant,
};

const REDIRECT_STATUS: u16 = 308;
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
    body_seen: u64,
    started: Instant,
    /// Counts the request as active while it is measured.
    active: Option<ActiveRequest>,
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
            body_seen: 0,
            started: Instant::now(),
            active: None,
        }
    }

    async fn request_filter(
        &self,
        session: &mut Session,
        ctx: &mut RequestContext,
    ) -> pingora_core::Result<bool> {
        request_identity::ensure_request_id(session.req_header_mut());
        if let Some(metrics) = &self.listener.metrics {
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
        if let Some(rejection) = self.tls_rejection(session, &snapshot, host_name) {
            let (status, message) = rejection;
            responses::plain(session, status, message, &[]).await?;
            return Ok(true);
        }

        let routing = &snapshot.routing;
        let entry = routing
            .lookup(host_name)
            .filter(|entry| routing.site(entry.site).serves(&self.listener.id));
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
        let route = site.route(route_index);
        let gates: Vec<usize> = site.security.into_iter().chain(route.security).collect();
        if !gates.is_empty() {
            let mut admission = Admission::default();
            let candidate = Candidate {
                client: ctx.client.unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
                header: session.req_header(),
                path: &path,
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
                refuse(session, refusal).await?;
                return Ok(true);
            }
            if let Some(timeout) = admission.body_timeout {
                session.set_read_timeout(Some(timeout));
            }
            ctx.admission = admission;
        }
        match &route.target {
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
        let key = pool.hash.extract(
            session.req_header(),
            ctx.client
                .map(|client| SocketAddr::new(client, 0))
                .or_else(|| client_address(session)),
        );
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
                close: !pool.keepalive,
            },
        )?;
        if let Some(client) = ctx.client {
            upstream_request.insert_header(X_REAL_IP.clone(), client.to_string())?;
        }
        Ok(())
    }

    async fn request_body_filter(
        &self,
        _session: &mut Session,
        body: &mut Option<bytes::Bytes>,
        _end_of_stream: bool,
        ctx: &mut RequestContext,
    ) -> pingora_core::Result<()> {
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
        let sent_at = ctx.sent_at.take();
        self.measure_upstream(
            &session.req_header().method,
            ctx,
            sent_at,
            None,
            Some(&error),
        );
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

    /// Pingora's answers to failed requests, except that a request body
    /// that stops arriving gets 408 (RFC 9110 §15.5.9) instead of 400.
    async fn fail_to_proxy(
        &self,
        session: &mut Session,
        error: &Error,
        _ctx: &mut RequestContext,
    ) -> FailToProxy {
        let code = match (error.etype(), error.esource()) {
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
        if code > 0 {
            if let Err(failure) = session.respond_error(code).await {
                tracing::debug!(%failure, "the error response did not reach the client");
            }
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
        let served = Served {
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
fn facts<'a>(session: &'a Session, host: &'a str, path: &'a str, tls: bool) -> Facts<'a> {
    Facts {
        host,
        uri: path,
        method: session.req_header().method.as_str(),
        scheme: if tls { "https" } else { "http" },
        client_ip: client_address(session).map(|address| address.ip()),
        headers: &session.req_header().headers,
        upstream: None,
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
