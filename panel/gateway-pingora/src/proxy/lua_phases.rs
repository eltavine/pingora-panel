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
    log_files::Destination,
    lua::{self, Hook, Hooks, LuaPlan, SessionHost, PROXIED_BODY_LIMIT},
    responses,
};
use async_trait::async_trait;
use bytes::Bytes;
use chrono::Utc;
use http::{header, Method};
use panel_errors::PanelError;
use panel_ir::LuaFallback;
use panel_lua::{Balancer, Chunk, NoHost, Outcome, PeerTimeouts, Scripts};
use pingora_core::{
    modules::http::{HttpModule, HttpModuleBuilder, Module},
    Error, ErrorType,
};
use pingora_http::ResponseHeader;
use pingora_proxy::Session;
use std::{
    any::Any,
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
}

/// Closes the connection without an answer, as nginx's 444 does.
pub(super) fn abort() -> Box<Error> {
    Error::explain(ErrorType::HTTPStatus(444), "a script closed the connection")
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
    /// Counts and measures a run, and writes what its script logged, its
    /// failure, a slow run and, when debugging, the run itself.
    fn finished(&self, hook: &Hook, scripts: &Scripts, outcome: &Outcome, elapsed: Duration) {
        let phase = hook.phase().name();
        let labels = &self.snapshot.labels;
        let site = self.site.and_then(|site| labels.site(site));
        let route = self
            .site
            .zip(self.route)
            .and_then(|(site, route)| labels.route(site, route));
        let slow = elapsed >= hook.slow;
        let result = match outcome {
            Outcome::Failed(failure) => failure.kind.name(),
            _ => "ok",
        };
        if let Some(metrics) = &self.listener.metrics {
            metrics.lua_run(site.clone(), route.clone(), phase, result, elapsed, slow);
        }
        let (logs, dropped, request_id) = {
            let mut exchange = scripts.exchange();
            (
                std::mem::take(&mut exchange.logs),
                std::mem::take(&mut exchange.dropped_logs),
                exchange.connection.request_id.clone(),
            )
        };
        let place = LuaPlace {
            request_id: (!request_id.is_empty()).then_some(request_id.as_str()),
            listener: &self.listener.id,
            site: site.as_deref(),
            route: route.as_deref(),
            phase,
            script: &hook.script,
        };
        let logging = &self.snapshot.logging;
        let write = |level: &str, message: &str| {
            if level == "error" || level == "warn" {
                tracing::warn!(event = "lua", phase, script = %hook.script, level, message);
            } else {
                tracing::debug!(event = "lua", phase, script = %hook.script, level, message);
            }
            if let Some(logs) = &self.listener.logs {
                logs.send(
                    Destination::Errors,
                    access_log::lua(&place, level, message, logging, Utc::now()),
                    logging.files,
                );
            }
        };
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

impl PanelProxy {
    fn lua_plan(ctx: &RequestContext) -> Option<Arc<LuaPlan>> {
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
        ctx: &RequestContext,
        plan: &LuaPlan,
        path: &str,
        host: &str,
    ) -> Scripts {
        if let Some(scripts) = take_scripts(session) {
            lua::sync_request(&mut scripts.exchange(), session.req_header(), path);
            return scripts;
        }
        let peer = client_address(session);
        let client = ctx
            .client
            .map(|ip| SocketAddr::new(ip, peer.map_or(0, |peer| peer.port())))
            .or(peer);
        let server = session
            .server_addr()
            .and_then(|address| address.as_inet())
            .copied();
        plan.runtime.scripts(lua::exchange(
            session.req_header(),
            path,
            client,
            server,
            self.listener.tls,
            host,
            ctx.started,
        ))
    }

    /// Readies the header filter of the request's route, or of its site,
    /// for the response the request gets.
    pub(super) fn lua_prepare_header_filter(
        &self,
        session: &mut Session,
        ctx: &RequestContext,
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
        let outcome = scripts
            .run(hook.handler, &mut SessionHost { session, limit })
            .await;
        report.finished(hook, &scripts, &outcome, started.elapsed());
        match outcome {
            Outcome::Continue | Outcome::Respond => {
                let (jump, body) = {
                    let mut exchange = scripts.exchange();
                    let changes = exchange.changes();
                    let body = changes
                        .body
                        .then(|| exchange.request.body.clone())
                        .flatten();
                    lua::apply_request(session.req_header_mut(), &mut exchange)
                        .map_err(internal)?;
                    (changes.jump, body)
                };
                if body.is_some() {
                    ctx.lua_body = body;
                }
                put_scripts(session, scripts);
                if outcome == Outcome::Respond {
                    self.lua_answer(session, ctx).await?;
                    return Ok(LuaStep::Done);
                }
                Ok(LuaStep::Go { jump })
            }
            Outcome::Abort => {
                put_scripts(session, scripts);
                Err(abort())
            }
            Outcome::Failed(_) => {
                put_scripts(session, scripts);
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
