//! The request path's Lua phases (ADR 0039): each takes the exchange from
//! the request, runs its hook and gives the request what the run changed,
//! or falls back as the hook says when it failed.
//!
//! A request's scripts live in its [`LuaModule`] between phases. The module
//! also runs the header filter, so that every response the request gets
//! passes it: proxied, made by a script, or made by the gateway.

use super::{client_address, ListenerContext, PanelProxy, RequestContext};
use crate::{
    access_log::{self, LuaPlace},
    adapter::PreparedPingoraSnapshot,
    certificates::{Handshake, TlsVersion},
    log_files::Destination,
    lua::{self, Hook, Hooks, LuaPlan, SessionHost, Variable, PROXIED_BODY_LIMIT},
    responses,
};
use async_trait::async_trait;
use bytes::Bytes;
use chrono::Utc;
use http::{header, Method};
use panel_errors::PanelError;
use panel_ir::LuaFallback;
use panel_lua::{
    Balancer, Capture, Captured, Chunk, Host, NoHost, Outcome, Output, PeerTimeouts, Scripts,
};
use pingora_core::{
    modules::http::{HttpModule, HttpModuleBuilder, Module},
    upstreams::peer::{HttpPeer, Peer},
    utils::tls::CertKey,
    Error, ErrorType,
};
use pingora_http::ResponseHeader;
use pingora_proxy::Session;
use std::{
    any::Any,
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};

/// Bytes a script may read of a request it answers itself.
const ANSWERED_BODY_LIMIT: usize = 16 << 20;

/// What the request path does after a request phase's handler.
pub(super) enum LuaStep {
    /// Goes on, choosing the route again when the script asked to.
    Go { jump: bool },
    /// The request was answered.
    Done,
    /// `ngx.exec`: the request starts over with its new URI.
    Redirect,
}

/// Closes the connection without an answer, as nginx's 444 does.
pub(super) fn abort() -> Box<Error> {
    Error::explain(ErrorType::HTTPStatus(444), "a script closed the connection")
}

/// Ends a request whose client closed the connection while a script
/// watched for it, logged as nginx's 499.
pub(super) fn client_closed() -> Box<Error> {
    Error::explain(
        ErrorType::HTTPStatus(499),
        "the client closed the connection",
    )
}

fn internal(error: PanelError) -> Box<Error> {
    Error::explain(ErrorType::InternalError, error.message)
}

fn failed(status: u16, what: &str) -> Box<Error> {
    Error::explain(
        ErrorType::HTTPStatus(status),
        format!("a {what} script failed"),
    )
}

/// The status a failed hook answers with, `None` to go on.
fn fallback_status(hook: &Hook, failure: u16) -> Option<u16> {
    match hook.fallback {
        LuaFallback::Continue => None,
        LuaFallback::Fail => Some(failure),
        LuaFallback::Status { status } => Some(status),
    }
}

/// The request's path, normalized, as the request phases saw it.
fn normalized(session: &Session) -> String {
    let path = session.req_header().uri.path();
    panel_routing::path::normalize(path).map_or_else(|| path.to_owned(), |path| path.into_owned())
}

/// Where a run's measurements and messages go.
#[derive(Clone)]
pub(super) struct Report {
    listener: Arc<ListenerContext>,
    snapshot: Arc<PreparedPingoraSnapshot>,
    site: Option<usize>,
    route: Option<usize>,
}

impl Report {
    /// Where the runs of a site's scripts that run for no request go.
    pub(super) fn site(
        listener: Arc<ListenerContext>,
        snapshot: Arc<PreparedPingoraSnapshot>,
        site: usize,
    ) -> Self {
        Self {
            listener,
            snapshot,
            site: Some(site),
            route: None,
        }
    }

    /// Where the runs of the scripts of `http` that run for no request go.
    pub(super) fn program(
        listener: Arc<ListenerContext>,
        snapshot: Arc<PreparedPingoraSnapshot>,
    ) -> Self {
        Self {
            listener,
            snapshot,
            site: None,
            route: None,
        }
    }

    /// Writes `message` of `hook`'s script, which ran for `request_id`.
    pub(super) fn write(&self, hook: &Hook, request_id: &str, level: &str, message: &str) {
        let phase = hook.phase().name();
        let labels = &self.snapshot.labels;
        let site = self.site.and_then(|site| labels.site(site));
        let route = self
            .site
            .zip(self.route)
            .and_then(|(site, route)| labels.route(site, route));
        if level == "error" || level == "warn" {
            tracing::warn!(event = "lua", phase, script = %hook.script, level, message);
        } else {
            tracing::debug!(event = "lua", phase, script = %hook.script, level, message);
        }
        if let Some(logs) = &self.listener.logs {
            let place = LuaPlace {
                request_id: (!request_id.is_empty()).then_some(request_id),
                listener: &self.listener.id,
                site: site.as_deref(),
                route: route.as_deref(),
                phase,
                script: &hook.script,
            };
            let logging = &self.snapshot.logging;
            logs.send(
                Destination::Errors,
                access_log::lua(&place, level, message, logging, Utc::now()),
                logging.files,
            );
        }
    }

    /// Counts and measures a run, and writes what its script logged, its
    /// failure, a slow run and, when debugging, the run itself.
    pub(super) fn finished(
        &self,
        hook: &Hook,
        scripts: &Scripts,
        outcome: &Outcome,
        elapsed: Duration,
    ) {
        let phase = hook.phase().name();
        let slow = elapsed >= hook.slow;
        let result = match outcome {
            Outcome::Failed(failure) => failure.kind.name(),
            _ => "ok",
        };
        if let Some(metrics) = &self.listener.metrics {
            let labels = &self.snapshot.labels;
            let site = self.site.and_then(|site| labels.site(site));
            let route = self
                .site
                .zip(self.route)
                .and_then(|(site, route)| labels.route(site, route));
            metrics.lua_run(site, route, phase, result, elapsed, slow);
        }
        let (logs, dropped, request_id) = {
            let mut exchange = scripts.exchange();
            (
                std::mem::take(&mut exchange.logs),
                std::mem::take(&mut exchange.dropped_logs),
                exchange.connection.request_id.clone(),
            )
        };
        let write = |level: &str, message: &str| self.write(hook, &request_id, level, message);
        for entry in &logs {
            write(entry.level.name(), &entry.message);
        }
        if dropped > 0 {
            write(
                "warn",
                &format!("{dropped} more messages of the run were dropped"),
            );
        }
        if let Outcome::Failed(failure) = outcome {
            write(
                "error",
                &format!(
                    "{phase} handler {} failed ({}): {}",
                    hook.script,
                    failure.kind.name(),
                    failure.message
                ),
            );
        }
        let millis = elapsed.as_secs_f64() * 1000.0;
        if slow {
            write(
                "warn",
                &format!("{phase} handler {} took {millis:.3} ms", hook.script),
            );
        }
        if hook.debug {
            write(
                "debug",
                &format!(
                    "{phase} handler {} ran in {millis:.3} ms: {result}",
                    hook.script
                ),
            );
        }
    }
}

/// How `chain` verifies for `peer` as OpenSSL's verify codes say it: 0 when
/// it does, with the pool's trust anchors or else the system's.
fn upstream_verification(
    peer: &HttpPeer,
    chain: &[rustls_pki_types::CertificateDer<'static>],
) -> i64 {
    use rustls::{client::danger::ServerCertVerifier, CertificateError};
    /// X509_V_ERR_UNSPECIFIED.
    const UNSPECIFIED: i64 = 1;
    let Some((leaf, intermediates)) = chain.split_first() else {
        return UNSPECIFIED;
    };
    if intermediates.is_empty() && crate::certificates::self_issued(leaf) {
        // X509_V_ERR_DEPTH_ZERO_SELF_SIGNED_CERT.
        return 18;
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier: Arc<dyn ServerCertVerifier> = match peer.options.ca.as_deref() {
        Some(anchors) => {
            let mut roots = rustls::RootCertStore::empty();
            for anchor in anchors {
                let _ = roots.add(rustls_pki_types::CertificateDer::from(
                    anchor.borrow_raw_cert().clone(),
                ));
            }
            match rustls::client::WebPkiServerVerifier::builder_with_provider(
                Arc::new(roots),
                provider,
            )
            .build()
            {
                Ok(verifier) => verifier,
                Err(_) => return UNSPECIFIED,
            }
        }
        None => match rustls_platform_verifier::Verifier::new(provider) {
            Ok(verifier) => Arc::new(verifier),
            Err(_) => return UNSPECIFIED,
        },
    };
    let name = if peer.sni().is_empty() {
        match peer.address().as_inet() {
            Some(address) => rustls_pki_types::ServerName::IpAddress(address.ip().into()),
            None => return UNSPECIFIED,
        }
    } else {
        match rustls_pki_types::ServerName::try_from(peer.sni().to_owned()) {
            Ok(name) => name,
            Err(_) => return UNSPECIFIED,
        }
    };
    let verified = verifier.verify_server_cert(
        leaf,
        intermediates,
        &name,
        &[],
        rustls_pki_types::UnixTime::now(),
    );
    match verified {
        Ok(_) => 0,
        Err(rustls::Error::InvalidCertificate(error)) => match error {
            CertificateError::NotValidYet | CertificateError::NotValidYetContext { .. } => 9,
            CertificateError::Expired | CertificateError::ExpiredContext { .. } => 10,
            CertificateError::BadSignature => 7,
            CertificateError::UnknownIssuer => 20,
            CertificateError::Revoked => 23,
            CertificateError::InvalidPurpose | CertificateError::InvalidPurposeContext { .. } => 26,
            CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. } => {
                62
            }
            _ => UNSPECIFIED,
        },
        Err(_) => UNSPECIFIED,
    }
}

/// Why `chain` and `key` cannot be presented, if they cannot.
fn unusable(chain: &[Vec<u8>], key: &[u8]) -> Option<String> {
    if chain
        .iter()
        .any(|certificate| x509_parser::parse_x509_certificate(certificate).is_err())
    {
        return Some("the certificate chain holds what is not an X.509 certificate".into());
    }
    let key = match rustls_pki_types::PrivateKeyDer::try_from(key.to_vec()) {
        Ok(key) => key,
        Err(error) => return Some(format!("the private key cannot be used: {error}")),
    };
    let signing = match rustls::crypto::ring::sign::any_supported_type(&key) {
        Ok(signing) => signing,
        Err(error) => return Some(format!("the private key cannot be used: {error}")),
    };
    let chain = chain
        .iter()
        .map(|certificate| rustls_pki_types::CertificateDer::from(certificate.clone()))
        .collect();
    rustls::sign::CertifiedKey::new(chain, signing)
        .keys_match()
        .err()
        .map(|error| format!("the private key does not match the certificate: {error}"))
}

/// A client's request as a handler that may answer it runs: what the
/// handler sends before it ends goes out through the header and body
/// filters, which run on the handler's VM with its `ngx.ctx`.
struct StreamingHost<'a> {
    host: SessionHost<'a>,
    proxy: &'a PanelProxy,
    ctx: &'a mut RequestContext,
    plan: Arc<LuaPlan>,
}

#[async_trait]
impl Host for StreamingHost<'_> {
    async fn read_body(&mut self, limit: usize) -> Result<Bytes, String> {
        self.host.read_body(limit).await
    }

    async fn read_body_chunk(&mut self) -> Result<Option<Bytes>, String> {
        self.host.read_body_chunk().await
    }

    async fn closed(&mut self) {
        self.host.closed().await;
    }

    async fn capture(&mut self, requests: Vec<Capture>) -> Result<Vec<Captured>, String> {
        self.host.capture(requests).await
    }

    /// Answers to HTTP/1.0 requests are buffered, so that they carry a
    /// Content-Length, as nginx's `lua_http10_buffering on` has them.
    fn streams(&self) -> bool {
        self.host.session.req_header().version != http::Version::HTTP_10
    }

    async fn send(&mut self, output: Output) -> Result<(), String> {
        let session = &mut *self.host.session;
        if let Some(exchange) = output.header {
            let filters = self.plan.runtime.scripts_sharing(*exchange, &output.share);
            let chunked = session.req_header().version != http::Version::HTTP_10;
            let header = lua::streamed_header(&filters.exchange(), chunked)
                .map_err(|error| error.message)?;
            put_scripts(session, filters);
            session
                .write_response_header(Box::new(header), false)
                .await
                .map_err(|error| error.to_string())?;
        }
        let mut chunk = Some(output.body);
        if let Some(hook) = PanelProxy::lua_hook(self.ctx, |hooks| &hooks.body_filter) {
            self.proxy
                .lua_body_filter(session, self.ctx, &hook, &mut chunk, output.last)
                .await
                .map_err(|error| error.to_string())?;
        }
        let chunk = chunk.filter(|chunk| !chunk.is_empty());
        if chunk.is_some() || output.last {
            session
                .write_response_body(chunk, output.last)
                .await
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    async fn read_raw(&mut self) -> Result<Option<Bytes>, String> {
        self.host
            .session
            .read_request_body()
            .await
            .map_err(|error| format!("the client connection could not be read: {error}"))
    }

    async fn write_raw(&mut self, data: Bytes) -> Result<(), String> {
        self.host
            .session
            .as_downstream_mut()
            .write_response_body(data, false)
            .await
            .map_err(|error| format!("the client connection could not be written: {error}"))
    }
}

/// A request's scripts between its phases, and its header filter.
#[derive(Default)]
pub(crate) struct LuaModule {
    scripts: Option<Scripts>,
    header_filter: Option<(Hook, Report)>,
}

#[async_trait]
impl HttpModule for LuaModule {
    async fn response_header_filter(
        &mut self,
        response: &mut ResponseHeader,
        _end_of_stream: bool,
    ) -> pingora_core::Result<()> {
        // A request's header filter runs once, on the response it gets first.
        let Some((hook, report)) = self.header_filter.take() else {
            return Ok(());
        };
        let Some(scripts) = self.scripts.as_mut() else {
            return Ok(());
        };
        {
            let mut exchange = scripts.exchange();
            exchange.response.status = response.status.as_u16();
            exchange.response.headers.clone_from(&response.headers);
        }
        let started = Instant::now();
        let outcome = scripts.run(hook.handler, &mut NoHost).await;
        report.finished(&hook, scripts, &outcome, started.elapsed());
        match outcome {
            Outcome::Continue | Outcome::Respond => {
                lua::apply_response(response, &mut scripts.exchange()).map_err(internal)
            }
            Outcome::Abort => Err(abort()),
            Outcome::Failed(_) => match fallback_status(&hook, 500) {
                None => Ok(()),
                Some(status) => Err(failed(status, "header filter")),
            },
        }
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

pub(crate) struct LuaModuleBuilder;

impl HttpModuleBuilder for LuaModuleBuilder {
    /// Before HTTP policies, as nginx runs Lua header filters before
    /// `add_header`.
    fn order(&self) -> i16 {
        100
    }

    fn init(&self) -> Module {
        Box::new(LuaModule::default())
    }
}

fn take_scripts(session: &mut Session) -> Option<Scripts> {
    session
        .downstream_modules_ctx
        .get_mut::<LuaModule>()?
        .scripts
        .take()
}

fn put_scripts(session: &mut Session, scripts: Scripts) {
    if let Some(module) = session.downstream_modules_ctx.get_mut::<LuaModule>() {
        module.scripts = Some(scripts);
    }
}

/// Runs `run` on the request's variables: its scripts' once it has any,
/// otherwise `variables`, the ones `set` gave it before.
pub(super) fn with_variables<R>(
    session: &Session,
    variables: &HashMap<String, String>,
    run: impl FnOnce(&HashMap<String, String>) -> R,
) -> R {
    match session
        .downstream_modules_ctx
        .get::<LuaModule>()
        .and_then(|module| module.scripts.as_ref())
    {
        Some(scripts) => run(&scripts.exchange().variables),
        None => run(variables),
    }
}

impl PanelProxy {
    pub(super) fn lua_plan(ctx: &RequestContext) -> Option<Arc<LuaPlan>> {
        ctx.snapshot
            .as_ref()?
            .lua
            .clone()
            .filter(|plan| !plan.disabled)
    }

    fn report(&self, ctx: &RequestContext) -> Option<Report> {
        Some(Report {
            listener: Arc::clone(&self.listener),
            snapshot: Arc::clone(ctx.snapshot.as_ref()?),
            site: ctx.site,
            route: ctx.route,
        })
    }

    /// The hook of the request's route, or of its site before a route was
    /// chosen.
    pub(super) fn lua_hook(
        ctx: &RequestContext,
        pick: fn(&Hooks) -> &Option<Hook>,
    ) -> Option<Hook> {
        Self::lua_plan(ctx)?;
        let snapshot = ctx.snapshot.as_ref()?;
        let site = snapshot.routing.site(ctx.site?);
        let hooks = match ctx.route {
            Some(route) => &site.route(route).lua,
            None => &site.lua,
        };
        pick(hooks).clone()
    }

    /// The request's scripts, brought up to date with the request.
    fn lua_scripts(
        &self,
        session: &mut Session,
        ctx: &mut RequestContext,
        plan: &LuaPlan,
        path: &str,
        host: &str,
    ) -> Scripts {
        if let Some(scripts) = take_scripts(session) {
            lua::sync_request(&mut scripts.exchange(), session.req_header(), path);
            return scripts;
        }
        let variables = std::mem::take(&mut ctx.variables);
        let peer = client_address(session);
        let client = ctx
            .client
            .map(|ip| SocketAddr::new(ip, peer.map_or(0, |peer| peer.port())))
            .or(peer);
        let server = session
            .server_addr()
            .and_then(|address| address.as_inet())
            .copied();
        let mut exchange = lua::exchange(
            session.req_header(),
            path,
            client,
            server,
            self.listener.tls,
            host,
            ctx.started,
        );
        if let Some(handshake) = session
            .digest()
            .and_then(|digest| digest.ssl_digest.as_ref())
            .and_then(|digest| digest.extension.get::<Handshake>())
        {
            exchange
                .handshake
                .server_name
                .clone_from(&handshake.server_name);
            exchange.handshake.version = handshake.version.map(TlsVersion::number);
            if let Some((verified, chain)) = &handshake.client {
                exchange.handshake.client_verify = Some(verified.clone());
                exchange.handshake.client_chain.clone_from(chain);
            }
        }
        if let Some(digest) = session
            .digest()
            .and_then(|digest| digest.ssl_digest.as_ref())
            .filter(|digest| !digest.cipher.is_empty())
        {
            exchange.handshake.cipher = Some(panel_ir::tls::openssl_name(&digest.cipher));
        }
        let scripts = match ctx.subrequest.as_ref() {
            Some(subrequest) => {
                let scripts = match &subrequest.share {
                    Some(share) => plan.runtime.scripts_sharing(exchange, share),
                    None => plan.runtime.scripts(exchange),
                };
                scripts.exchange().set_subrequest();
                scripts
            }
            None => plan.runtime.scripts(exchange),
        };
        scripts.exchange().variables = variables;
        scripts
    }

    /// Sets `variables` in order as a site's or route's first phase begins:
    /// a value, or what its `set_by_lua` handler returns. Handlers do not
    /// run with `lua off`.
    pub(super) async fn lua_variables(
        &self,
        session: &mut Session,
        ctx: &mut RequestContext,
        variables: &[Variable],
        path: &str,
        host: &str,
    ) -> pingora_core::Result<LuaStep> {
        for variable in variables {
            let Some(hook) = &variable.hook else {
                let name = variable.name.to_string();
                let value = variable.value.to_string();
                match take_scripts(session) {
                    Some(scripts) => {
                        scripts.exchange().variables.insert(name, value);
                        put_scripts(session, scripts);
                    }
                    None => {
                        ctx.variables.insert(name, value);
                    }
                }
                continue;
            };
            let (Some(plan), Some(report)) = (Self::lua_plan(ctx), self.report(ctx)) else {
                continue;
            };
            let tls = self.listener.tls;
            let args = with_variables(session, &ctx.variables, |known| {
                let facts = super::facts(session, host, path, tls, known, &ctx.request_uri);
                variable
                    .args
                    .iter()
                    .map(|arg| {
                        crate::template::Template::parse(arg).map_or_else(
                            |_| String::new(),
                            |template| {
                                String::from_utf8_lossy(&template.render(&facts)).into_owned()
                            },
                        )
                    })
                    .collect()
            });
            let mut scripts = self.lua_scripts(session, ctx, &plan, path, host);
            let started = Instant::now();
            let outcome = scripts
                .set(hook.handler, &mut NoHost, &variable.name, args)
                .await;
            report.finished(hook, &scripts, &outcome, started.elapsed());
            put_scripts(session, scripts);
            if let Outcome::Failed(_) = outcome {
                if let Some(status) = fallback_status(hook, 500) {
                    responses::send(
                        session,
                        status,
                        &[
                            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
                            (header::CACHE_CONTROL, "no-store"),
                        ],
                        lua::default_body(status),
                    )
                    .await?;
                    return Ok(LuaStep::Done);
                }
            }
        }
        Ok(LuaStep::Go { jump: false })
    }

    /// Readies the header filter of the request's route, or of its site,
    /// for the response the request gets.
    pub(super) fn lua_prepare_header_filter(
        &self,
        session: &mut Session,
        ctx: &mut RequestContext,
        path: &str,
        host: &str,
    ) {
        let hook = Self::lua_hook(ctx, |hooks| &hooks.header_filter);
        let (Some(hook), Some(plan), Some(report)) = (hook, Self::lua_plan(ctx), self.report(ctx))
        else {
            if let Some(module) = session.downstream_modules_ctx.get_mut::<LuaModule>() {
                module.header_filter = None;
            }
            return;
        };
        let scripts = self.lua_scripts(session, ctx, &plan, path, host);
        if let Some(module) = session.downstream_modules_ctx.get_mut::<LuaModule>() {
            module.scripts = Some(scripts);
            module.header_filter = Some((hook, report));
        }
    }

    /// Runs a hook of the rewrite, access or content phase.
    pub(super) async fn lua_request(
        &self,
        session: &mut Session,
        ctx: &mut RequestContext,
        hook: &Hook,
        path: &str,
        host: &str,
        proxied: bool,
    ) -> pingora_core::Result<LuaStep> {
        let (Some(plan), Some(report)) = (Self::lua_plan(ctx), self.report(ctx)) else {
            return Ok(LuaStep::Go { jump: false });
        };
        let mut scripts = self.lua_scripts(session, ctx, &plan, path, host);
        let limit = if proxied {
            PROXIED_BODY_LIMIT
        } else {
            ANSWERED_BODY_LIMIT
        };
        let started = Instant::now();
        let depth = ctx
            .subrequest
            .as_ref()
            .map_or(0, |subrequest| subrequest.depth);
        let host = SessionHost {
            session,
            limit,
            streamed: 0,
            depth,
        };
        // A subrequest's response goes back to the script that made it.
        let outcome = if ctx.subrequest.is_some() {
            scripts.run(hook.handler, &mut { host }).await
        } else {
            let mut host = StreamingHost {
                host,
                proxy: self,
                ctx,
                plan: Arc::clone(&plan),
            };
            scripts.run(hook.handler, &mut host).await
        };
        report.finished(hook, &scripts, &outcome, started.elapsed());
        match outcome {
            Outcome::Continue | Outcome::Respond => {
                let (jump, redirected, body) = {
                    let mut exchange = scripts.exchange();
                    let changes = exchange.changes();
                    let body = changes
                        .body
                        .then(|| exchange.request.body.clone())
                        .flatten();
                    lua::apply_request(session.req_header_mut(), &mut exchange)
                        .map_err(internal)?;
                    ctx.named = exchange.take_named();
                    (changes.jump, exchange.redirected(), body)
                };
                if body.is_some() {
                    ctx.lua_body = body;
                }
                put_scripts(session, scripts);
                if redirected {
                    return Ok(LuaStep::Redirect);
                }
                if outcome == Outcome::Respond {
                    self.lua_answer(session, ctx).await?;
                    return Ok(LuaStep::Done);
                }
                Ok(LuaStep::Go { jump })
            }
            Outcome::Abort => {
                let left = scripts.exchange().client_closed();
                put_scripts(session, scripts);
                Err(if left { client_closed() } else { abort() })
            }
            Outcome::Failed(_) => {
                let (streaming, ended) = {
                    let exchange = scripts.exchange();
                    (exchange.streaming(), exchange.ended())
                };
                put_scripts(session, scripts);
                // What the client got cannot be taken back: a response the
                // handler ended stays whole, one it left open is cut off.
                if streaming {
                    return if ended {
                        Ok(LuaStep::Done)
                    } else {
                        Err(abort())
                    };
                }
                match fallback_status(hook, 500) {
                    None => Ok(LuaStep::Go { jump: false }),
                    Some(status) => {
                        responses::send(
                            session,
                            status,
                            &[
                                (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
                                (header::CACHE_CONTROL, "no-store"),
                            ],
                            lua::default_body(status),
                        )
                        .await?;
                        Ok(LuaStep::Done)
                    }
                }
            }
        }
    }

    /// Sends the response a handler answered with, through the body filter
    /// of the request's route; the header filter runs as it is written.
    pub(super) async fn lua_answer(
        &self,
        session: &mut Session,
        ctx: &mut RequestContext,
    ) -> pingora_core::Result<()> {
        let Some(scripts) = take_scripts(session) else {
            return Ok(());
        };
        let streamed = {
            let mut exchange = scripts.exchange();
            exchange.streaming().then(|| {
                (
                    exchange.ended(),
                    Bytes::from(std::mem::take(&mut exchange.response.body)),
                )
            })
        };
        if let Some((ended, rest)) = streamed {
            put_scripts(session, scripts);
            if ended {
                return Ok(());
            }
            let mut chunk = Some(rest);
            if let Some(hook) = Self::lua_hook(ctx, |hooks| &hooks.body_filter) {
                self.lua_body_filter(session, ctx, &hook, &mut chunk, true)
                    .await?;
            }
            return session
                .write_response_body(chunk.filter(|chunk| !chunk.is_empty()), true)
                .await;
        }
        let (mut header, mut body) = {
            let mut exchange = scripts.exchange();
            let status = match exchange.response.status {
                0 => 200,
                status => status,
            };
            let printed = std::mem::take(&mut exchange.response.body);
            let body = if printed.is_empty() && status >= 400 {
                lua::default_body(status)
            } else {
                Bytes::from(printed)
            };
            (
                lua::response_header(&exchange, body.len()).map_err(internal)?,
                body,
            )
        };
        put_scripts(session, scripts);
        if let Some(hook) = Self::lua_hook(ctx, |hooks| &hooks.body_filter) {
            let mut chunk = Some(body);
            self.lua_body_filter(session, ctx, &hook, &mut chunk, true)
                .await?;
            body = chunk.unwrap_or_default();
            if header.headers.contains_key(header::CONTENT_LENGTH) {
                header.insert_header(header::CONTENT_LENGTH, body.len().to_string())?;
            }
        }
        let empty = body.is_empty()
            || session.req_header().method == Method::HEAD
            || matches!(header.status.as_u16(), 100..=199 | 204 | 304);
        session
            .write_response_header(Box::new(header), empty)
            .await?;
        if !empty {
            session.write_response_body(Some(body), true).await?;
        }
        Ok(())
    }

    /// `proxy_ssl_certificate_by_lua`: the certificate the request's TLS
    /// connection to `peer` presents when the upstream asks for one.
    pub(super) async fn lua_upstream_certificate(
        &self,
        session: &mut Session,
        ctx: &mut RequestContext,
        peer: &mut HttpPeer,
    ) -> pingora_core::Result<()> {
        if !peer.is_tls() {
            return Ok(());
        }
        let Some(hook) = Self::lua_hook(ctx, |hooks| &hooks.proxy_ssl_cert) else {
            return Ok(());
        };
        let (Some(plan), Some(report)) = (Self::lua_plan(ctx), self.report(ctx)) else {
            return Ok(());
        };
        let path = normalized(session);
        let mut scripts = self.lua_scripts(session, ctx, &plan, &path, "");
        let started = Instant::now();
        let outcome = scripts.run(hook.handler, &mut NoHost).await;
        report.finished(&hook, &scripts, &outcome, started.elapsed());
        let (chain, key, cleared, request_id) = {
            let mut exchange = scripts.exchange();
            let request_id = exchange.connection.request_id.clone();
            let handshake = &mut exchange.handshake;
            (
                handshake.chain.take(),
                handshake.key.take(),
                std::mem::take(&mut handshake.cleared),
                request_id,
            )
        };
        put_scripts(session, scripts);
        let refused = || failed(502, "proxy SSL certificate");
        match outcome {
            Outcome::Abort => return Err(refused()),
            Outcome::Failed(_) => {
                return match fallback_status(&hook, 502) {
                    None => Ok(()),
                    Some(status) => Err(failed(status, "proxy SSL certificate")),
                }
            }
            Outcome::Continue | Outcome::Respond => {}
        }
        let problem = match (chain, key) {
            (Some(chain), Some(key)) => match unusable(&chain, &key) {
                None => {
                    peer.client_cert_key = Some(Arc::new(CertKey::new(chain, key)));
                    return Ok(());
                }
                Some(problem) => problem,
            },
            (None, None) => {
                if cleared {
                    peer.client_cert_key = None;
                }
                return Ok(());
            }
            (Some(_), None) => "the script set a certificate without its private key".into(),
            (None, Some(_)) => "the script set a private key without its certificate".into(),
        };
        report.write(&hook, &request_id, "error", &problem);
        Err(refused())
    }

    /// `proxy_ssl_verify_by_lua`: whether the request's new TLS connection
    /// to `peer` may carry it, by what the upstream presented. A script
    /// that sets a verify result other than 0 refuses the connection.
    pub(super) async fn lua_upstream_verify(
        &self,
        session: &mut Session,
        ctx: &mut RequestContext,
        peer: &HttpPeer,
        digest: Option<&pingora_core::protocols::Digest>,
    ) -> pingora_core::Result<()> {
        let Some(hook) = Self::lua_hook(ctx, |hooks| &hooks.proxy_ssl_verify) else {
            return Ok(());
        };
        let (Some(plan), Some(report)) = (Self::lua_plan(ctx), self.report(ctx)) else {
            return Ok(());
        };
        let tls = digest.and_then(|digest| digest.ssl_digest.as_ref());
        let chain: Vec<rustls_pki_types::CertificateDer<'static>> = tls
            .and_then(|tls| {
                tls.extension
                    .get::<Vec<rustls_pki_types::CertificateDer<'static>>>()
            })
            .cloned()
            .unwrap_or_default();
        let version = tls
            .and_then(|tls| TlsVersion::parse(&tls.version))
            .map(TlsVersion::number);
        let verify_result = if peer.verify_cert() {
            0
        } else {
            upstream_verification(peer, &chain)
        };
        let path = normalized(session);
        let mut scripts = self.lua_scripts(session, ctx, &plan, &path, "");
        {
            let mut exchange = scripts.exchange();
            let upstream = &mut exchange.upstream_tls;
            upstream.chain = chain
                .iter()
                .map(|certificate| certificate.to_vec())
                .collect();
            upstream.version = version;
            upstream.verify_result = verify_result;
            upstream.verdict = None;
        }
        let started = Instant::now();
        let outcome = scripts.run(hook.handler, &mut NoHost).await;
        report.finished(&hook, &scripts, &outcome, started.elapsed());
        let (verdict, request_id) = {
            let mut exchange = scripts.exchange();
            let verdict = exchange.upstream_tls.verdict.take();
            exchange.upstream_tls = panel_lua::UpstreamTls::default();
            (verdict, exchange.connection.request_id.clone())
        };
        put_scripts(session, scripts);
        match outcome {
            Outcome::Abort => return Err(failed(502, "proxy SSL verify")),
            Outcome::Failed(_) => {
                return match fallback_status(&hook, 502) {
                    None => Ok(()),
                    Some(status) => Err(failed(status, "proxy SSL verify")),
                }
            }
            Outcome::Continue | Outcome::Respond => {}
        }
        match verdict {
            Some(code) if code != 0 => {
                report.write(
                    &hook,
                    &request_id,
                    "error",
                    &format!(
                        "the script refused the upstream's certificate (verify result {code})"
                    ),
                );
                Err(failed(502, "proxy SSL verify"))
            }
            _ => Ok(()),
        }
    }

    /// Runs the body filter on a piece of the response body. A filter that
    /// ends the body early drops what follows.
    pub(super) async fn lua_body_filter(
        &self,
        session: &mut Session,
        ctx: &mut RequestContext,
        hook: &Hook,
        body: &mut Option<Bytes>,
        end: bool,
    ) -> pingora_core::Result<()> {
        if ctx.lua_body_done {
            *body = None;
            return Ok(());
        }
        let (Some(plan), Some(report)) = (Self::lua_plan(ctx), self.report(ctx)) else {
            return Ok(());
        };
        let path = normalized(session);
        let mut scripts = self.lua_scripts(session, ctx, &plan, &path, "");
        scripts.exchange().chunk = Chunk {
            data: body.take().unwrap_or_default(),
            eof: end,
        };
        let started = Instant::now();
        let outcome = scripts.run(hook.handler, &mut NoHost).await;
        report.finished(hook, &scripts, &outcome, started.elapsed());
        let result = match outcome {
            Outcome::Abort => Err(abort()),
            Outcome::Failed(_) if !matches!(hook.fallback, LuaFallback::Continue) => {
                Err(failed(500, "body filter"))
            }
            _ => Ok(()),
        };
        {
            let exchange = scripts.exchange();
            *body = Some(exchange.chunk.data.clone());
            if exchange.chunk.eof && !end {
                ctx.lua_body_done = true;
            }
        }
        put_scripts(session, scripts);
        result
    }

    /// Runs the upstream's balancer: the address it chose, if it chose one,
    /// with the timeouts of the try.
    pub(super) async fn lua_balancer(
        &self,
        session: &mut Session,
        ctx: &mut RequestContext,
        hook: &Hook,
        upstream: &str,
    ) -> pingora_core::Result<Option<(SocketAddr, PeerTimeouts)>> {
        let (Some(plan), Some(report)) = (Self::lua_plan(ctx), self.report(ctx)) else {
            return Ok(None);
        };
        let path = normalized(session);
        let mut scripts = self.lua_scripts(session, ctx, &plan, &path, "");
        scripts.exchange().balancer = Balancer {
            upstream: upstream.to_owned(),
            tries: ctx.lua_tries,
            last_failure: ctx.lua_last_failure.clone(),
            peer: None,
            more_tries: None,
            timeouts: PeerTimeouts::default(),
        };
        let started = Instant::now();
        let outcome = scripts.run(hook.handler, &mut NoHost).await;
        report.finished(hook, &scripts, &outcome, started.elapsed());
        let result = match outcome {
            Outcome::Continue => {
                let mut exchange = scripts.exchange();
                let balancer = std::mem::take(&mut exchange.balancer);
                exchange.clear_changes();
                if let Some(more) = balancer.more_tries {
                    ctx.lua_more_tries = more;
                }
                match balancer.peer {
                    None => Ok(None),
                    Some(peer) => match peer.host.parse::<IpAddr>() {
                        Ok(ip) => Ok(Some((SocketAddr::new(ip, peer.port), balancer.timeouts))),
                        Err(_) => Err(Error::explain(
                            ErrorType::HTTPStatus(502),
                            format!(
                                "the balancer chose {}, which is not an IP address",
                                peer.host
                            ),
                        )),
                    },
                }
            }
            Outcome::Respond => {
                let status = match scripts.exchange().response.status {
                    0 => 502,
                    status => status,
                };
                Err(Error::explain(
                    ErrorType::HTTPStatus(status),
                    "the balancer ended the request",
                ))
            }
            Outcome::Abort => Err(abort()),
            Outcome::Failed(_) => match fallback_status(hook, 502) {
                None => Ok(None),
                Some(status) => Err(failed(status, "balancer")),
            },
        };
        ctx.lua_tries += 1;
        put_scripts(session, scripts);
        result
    }

    /// Runs the log phase once the response is sent.
    pub(super) async fn lua_log_phase(
        &self,
        session: &mut Session,
        ctx: &mut RequestContext,
        hook: &Hook,
        status: u16,
    ) {
        let (Some(plan), Some(report)) = (Self::lua_plan(ctx), self.report(ctx)) else {
            return;
        };
        let path = normalized(session);
        let mut scripts = self.lua_scripts(session, ctx, &plan, &path, "");
        {
            let mut exchange = scripts.exchange();
            let variables = &mut exchange.variables;
            variables.insert("status".into(), status.to_string());
            variables.insert(
                "request_time".into(),
                format!("{:.3}", ctx.started.elapsed().as_secs_f64()),
            );
            if let Some(address) = ctx.lua_peer.or_else(|| {
                ctx.upstream()
                    .map(|(pool, endpoint)| pool.endpoints[endpoint].address)
            }) {
                variables.insert("upstream_addr".into(), address.to_string());
            }
        }
        let started = Instant::now();
        let outcome = scripts.run(hook.handler, &mut NoHost).await;
        report.finished(hook, &scripts, &outcome, started.elapsed());
        put_scripts(session, scripts);
    }
}
