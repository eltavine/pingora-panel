//! Lua handlers in the data plane (ADR 0039): the runtime of a snapshot,
//! the hooks its sites, routes and upstreams run, and how a run's exchange
//! is taken from a request and given back to it.

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use http::{header, HeaderMap, HeaderValue, Method};
use panel_errors::{Diagnostic, ErrorCode, PanelError, Result};
use panel_ir::{LuaFallback, LuaHandler, LuaHandlers, LuaLogLevel, RouteAction, RuntimeSnapshot};
use panel_lua::{
    Connection, Exchange, Handler, HandlerId, Host, Limits, LogLevel, Permissions, Phase, Program,
    Request, Runtime, Settings, SharedStore, Source,
};
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, CONTROLS};
use pingora_http::{RequestHeader, ResponseHeader};
use pingora_proxy::Session;
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

const DEFAULT_TIME: Duration = Duration::from_millis(100);
const DEFAULT_WORK: u64 = 10_000_000;
const DEFAULT_SLOW: Duration = Duration::from_millis(10);
const DEFAULT_MEMORY: usize = 64 << 20;
/// Bytes of a request body the gateway holds for a request it proxies after
/// a script read it: what Pingora keeps to send the body again.
pub(crate) const PROXIED_BODY_LIMIT: usize = 64 << 10;

/// Bytes a decoded path is encoded back with when a script sets it: those
/// RFC 3986 §3.3 does not allow in a path segment.
const PATH: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// A handler as the request path runs it.
#[derive(Clone, Debug)]
pub(crate) struct Hook {
    pub handler: Handler,
    pub fallback: LuaFallback,
    pub slow: Duration,
    pub debug: bool,
    pub script: Arc<str>,
}

impl Hook {
    pub(crate) fn phase(&self) -> Phase {
        self.handler.phase
    }
}

/// The hooks a site or route runs, by phase.
#[derive(Clone, Debug, Default)]
pub(crate) struct Hooks {
    pub server_rewrite: Option<Hook>,
    pub rewrite: Option<Hook>,
    pub access: Option<Hook>,
    pub header_filter: Option<Hook>,
    pub body_filter: Option<Hook>,
    pub log: Option<Hook>,
}

/// Hooks by the IR identifier of what runs them.
#[derive(Debug, Default)]
pub(crate) struct HookIndex {
    pub sites: HashMap<String, Hooks>,
    pub routes: HashMap<String, Hooks>,
    pub contents: HashMap<String, Hook>,
    pub balancers: HashMap<String, Hook>,
}

/// A snapshot's VMs.
pub(crate) struct LuaPlan {
    pub runtime: Runtime,
    /// `lua off`: scripts stay but none runs.
    pub disabled: bool,
}

impl std::fmt::Debug for LuaPlan {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("LuaPlan")
            .field("disabled", &self.disabled)
            .finish_non_exhaustive()
    }
}

fn level(level: LuaLogLevel) -> LogLevel {
    match level {
        LuaLogLevel::Stderr => LogLevel::Stderr,
        LuaLogLevel::Emerg => LogLevel::Emerg,
        LuaLogLevel::Alert => LogLevel::Alert,
        LuaLogLevel::Crit => LogLevel::Crit,
        LuaLogLevel::Error => LogLevel::Err,
        LuaLogLevel::Warn => LogLevel::Warn,
        LuaLogLevel::Notice => LogLevel::Notice,
        LuaLogLevel::Info => LogLevel::Info,
        LuaLogLevel::Debug => LogLevel::Debug,
    }
}

struct Compiler<'a> {
    snapshot: &'a RuntimeSnapshot,
    builder: panel_lua::ProgramBuilder,
    handlers: HashMap<String, HandlerId>,
}

impl Compiler<'_> {
    fn hook(&mut self, handler: &LuaHandler, phase: Phase) -> Result<Hook> {
        let script = self
            .snapshot
            .lua
            .script(&handler.script_id)
            .ok_or_else(|| {
                PanelError::validation_failed(format!(
                    "a Lua handler names unknown script {}",
                    handler.script_id
                ))
            })?;
        let id = match self.handlers.get(&script.id) {
            Some(id) => *id,
            None => {
                let id = self.builder.handler(&Source::new(
                    script.file.clone(),
                    script.source.clone(),
                    script.line,
                ));
                self.handlers.insert(script.id.clone(), id);
                id
            }
        };
        Ok(Hook {
            handler: Handler {
                id,
                phase,
                limits: Limits {
                    time: match handler.time_limit_ms {
                        0 => DEFAULT_TIME,
                        millis => Duration::from_millis(millis),
                    },
                    work: match handler.work_limit {
                        0 => DEFAULT_WORK,
                        work => work,
                    },
                },
                permissions: Permissions {
                    body: handler.allow.body,
                    upstream: handler.allow.upstream,
                    network: handler.allow.network,
                },
                log_level: level(handler.log_level),
            },
            fallback: handler.on_error,
            slow: match handler.slow_threshold_ms {
                0 => DEFAULT_SLOW,
                millis => Duration::from_millis(millis),
            },
            debug: handler.debug,
            script: Arc::from(script.id.as_str()),
        })
    }

    fn hooks(&mut self, handlers: &LuaHandlers) -> Result<Hooks> {
        let mut hook = |handler: &Option<LuaHandler>, phase| {
            handler
                .as_ref()
                .map(|handler| self.hook(handler, phase))
                .transpose()
        };
        Ok(Hooks {
            server_rewrite: hook(&handlers.server_rewrite, Phase::ServerRewrite)?,
            rewrite: hook(&handlers.rewrite, Phase::Rewrite)?,
            access: hook(&handlers.access, Phase::Access)?,
            header_filter: hook(&handlers.header_filter, Phase::HeaderFilter)?,
            body_filter: hook(&handlers.body_filter, Phase::BodyFilter)?,
            log: hook(&handlers.log, Phase::Log)?,
        })
    }
}

/// Compiles `snapshot`'s scripts and starts `vms` VMs for them. Syntax
/// errors and failing `init_by_lua` refuse the snapshot.
pub(crate) fn compile(
    snapshot: &RuntimeSnapshot,
    store: &SharedStore,
    vms: usize,
) -> Result<(Option<Arc<LuaPlan>>, HookIndex)> {
    if !panel_engine::uses_lua(snapshot) {
        return Ok((None, HookIndex::default()));
    }
    let mut compiler = Compiler {
        snapshot,
        builder: Program::builder(),
        handlers: HashMap::new(),
    };
    for script in &snapshot.lua.scripts {
        if let Some(module) = &script.module {
            compiler.builder.module(
                module.clone(),
                &Source::new(script.file.clone(), script.source.clone(), script.line),
            );
        }
    }
    let mut index = HookIndex::default();
    for (phase, handler) in [
        (Phase::Init, &snapshot.lua.init),
        (Phase::InitWorker, &snapshot.lua.init_worker),
    ] {
        if let Some(handler) = handler {
            let hook = compiler.hook(handler, phase)?;
            let limits = hook.handler.limits;
            match phase {
                Phase::Init => compiler.builder.init(hook.handler.id),
                _ => compiler.builder.init_worker(hook.handler.id),
            };
            compiler.builder.init_limits(limits);
        }
    }
    for site in &snapshot.sites {
        if !site.lua.is_empty() {
            index
                .sites
                .insert(site.id.as_str().into(), compiler.hooks(&site.lua)?);
        }
    }
    for route in &snapshot.routes {
        if !route.lua.is_empty() {
            index
                .routes
                .insert(route.id.as_str().into(), compiler.hooks(&route.lua)?);
        }
        if let RouteAction::Lua { handler } = &route.action {
            let hook = compiler.hook(handler, Phase::Content)?;
            index.contents.insert(route.id.as_str().into(), hook);
        }
    }
    for pool in &snapshot.upstream_pools {
        if let Some(handler) = &pool.balancer {
            let hook = compiler.hook(handler, Phase::Balancer)?;
            index.balancers.insert(pool.id.as_str().into(), hook);
        }
    }
    for dict in &snapshot.lua.shared_dicts {
        compiler.builder.shared_dict(
            dict.name.clone(),
            usize::try_from(dict.capacity_bytes).unwrap_or(usize::MAX),
        );
    }
    let program = compiler.builder.build().map_err(|diagnostics| {
        PanelError::new(ErrorCode::VALIDATION_FAILED, "Lua scripts do not compile")
            .with_diagnostics(
                diagnostics
                    .into_iter()
                    .map(|diagnostic| {
                        Diagnostic::error(ErrorCode::VALIDATION_FAILED, diagnostic.to_string())
                            .with_resource(format!("lua:{}", diagnostic.source))
                    })
                    .collect(),
            )
    })?;
    let settings = Settings {
        vms: vms.max(1),
        memory: match snapshot.lua.memory_limit_bytes {
            0 => DEFAULT_MEMORY,
            bytes => usize::try_from(bytes).unwrap_or(usize::MAX),
        },
    };
    let (runtime, logs) = Runtime::start(&program, &settings, store).map_err(|failure| {
        PanelError::validation_failed(format!(
            "init_by_lua failed ({}): {}",
            failure.kind.name(),
            failure.message
        ))
    })?;
    for entry in logs {
        tracing::info!(event = "lua_log", phase = "init", level = entry.level.name(), message = %entry.message);
    }
    Ok((
        Some(Arc::new(LuaPlan {
            runtime,
            disabled: snapshot.lua.disabled,
        })),
        index,
    ))
}

/// `$uri`: the normalized path with every percent-encoding decoded.
pub(crate) fn decoded_path(path: &str) -> String {
    percent_decode_str(path).decode_utf8_lossy().into_owned()
}

/// What the request looks like now, for the next handler to see.
pub(crate) fn sync_request(exchange: &mut Exchange, header: &RequestHeader, path: &str) {
    let request = &mut exchange.request;
    request.method = header.method.as_str().to_owned();
    request.uri = decoded_path(path);
    request.request_uri = header
        .uri
        .path_and_query()
        .map_or("/", |value| value.as_str())
        .to_owned();
    request.args = header.uri.query().map(str::to_owned);
    request.version = header.version;
    request.headers.clone_from(&header.headers);
}

/// The exchange a request's first handler starts from.
pub(crate) fn exchange(
    header: &RequestHeader,
    path: &str,
    client: Option<SocketAddr>,
    server: Option<SocketAddr>,
    tls: bool,
    host: &str,
    started: Instant,
) -> Exchange {
    let mut exchange = Exchange::new(
        Request::default(),
        Connection {
            client,
            server,
            tls,
            server_name: host.to_owned(),
            request_id: crate::request_identity::request_id(&header.headers)
                .unwrap_or_default()
                .to_owned(),
            started: SystemTime::now()
                .checked_sub(started.elapsed())
                .unwrap_or_else(SystemTime::now),
        },
    );
    sync_request(&mut exchange, header, path);
    exchange
}

/// The fields to remove and the lines to append that turn `old` into `new`.
fn header_plan(
    old: &HeaderMap,
    new: &HeaderMap,
) -> (
    Vec<header::HeaderName>,
    Vec<(header::HeaderName, HeaderValue)>,
) {
    let changed = |name: &header::HeaderName| old.get_all(name).iter().ne(new.get_all(name).iter());
    let removed = old.keys().filter(|name| changed(name)).cloned().collect();
    let appended = new
        .keys()
        .filter(|name| changed(name))
        .flat_map(|name| {
            new.get_all(name)
                .iter()
                .map(move |value| (name.clone(), value.clone()))
        })
        .collect();
    (removed, appended)
}

fn header_error(error: impl std::fmt::Display) -> PanelError {
    PanelError::validation_failed(format!("a script set an invalid request: {error}"))
}

/// Gives the request line and header a handler changed back to the request.
/// Returns whether the path changed, so the route may be chosen again.
pub(crate) fn apply_request(header: &mut RequestHeader, exchange: &mut Exchange) -> Result<bool> {
    let changes = exchange.changes();
    let request = &exchange.request;
    if changes.method {
        let method = Method::from_bytes(request.method.as_bytes()).map_err(header_error)?;
        header.set_method(method);
    }
    if changes.uri || changes.args {
        let path = if changes.uri {
            utf8_percent_encode(&request.uri, PATH).to_string()
        } else {
            header.uri.path().to_owned()
        };
        let target = match &request.args {
            Some(args) if !args.is_empty() => format!("{path}?{args}"),
            _ => path,
        };
        header.set_uri(target.parse().map_err(header_error)?);
    }
    if changes.headers {
        let (removed, appended) = header_plan(&header.headers, &request.headers);
        for name in &removed {
            header.remove_header(name);
        }
        for (name, value) in appended {
            header.append_header(name, value).map_err(header_error)?;
        }
    }
    exchange.clear_changes();
    Ok(changes.uri)
}

/// Gives the status and header a header filter changed back to the
/// response.
pub(crate) fn apply_response(header: &mut ResponseHeader, exchange: &mut Exchange) -> Result<()> {
    let changes = exchange.changes();
    if changes.status && exchange.response.status != 0 {
        header
            .set_status(exchange.response.status)
            .map_err(header_error)?;
    }
    if changes.response_headers {
        let (removed, appended) = header_plan(&header.headers, &exchange.response.headers);
        for name in &removed {
            header.remove_header(name);
        }
        for (name, value) in appended {
            header.append_header(name, value).map_err(header_error)?;
        }
    }
    exchange.clear_changes();
    Ok(())
}

/// The response a handler answered with, as Pingora sends it.
pub(crate) fn response_header(exchange: &Exchange, body: usize) -> Result<ResponseHeader> {
    let status = match exchange.response.status {
        0 => 200,
        status => status,
    };
    let mut response = ResponseHeader::build(status, Some(exchange.response.headers.len() + 2))
        .map_err(header_error)?;
    for (name, value) in &exchange.response.headers {
        response
            .append_header(name.clone(), value.clone())
            .map_err(header_error)?;
    }
    if !matches!(status, 100..=199 | 204 | 304) {
        if !response.headers.contains_key(header::CONTENT_TYPE) {
            response
                .insert_header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                .map_err(header_error)?;
        }
        response
            .insert_header(header::CONTENT_LENGTH, body.to_string())
            .map_err(header_error)?;
    }
    Ok(response)
}

/// What a request a handler answered without output gets: nginx's default
/// page for the status, as plain text.
pub(crate) fn default_body(status: u16) -> Bytes {
    let reason = http::StatusCode::from_u16(status)
        .ok()
        .and_then(|status| status.canonical_reason())
        .unwrap_or("");
    Bytes::from(format!("{status} {reason}\n"))
}

/// Reads the request body for `ngx.req.read_body`.
pub(crate) struct SessionHost<'a> {
    pub session: &'a mut Session,
    /// The most the gateway holds: less for requests it proxies afterwards.
    pub limit: usize,
}

#[async_trait]
impl Host for SessionHost<'_> {
    async fn read_body(&mut self, limit: usize) -> std::result::Result<Bytes, String> {
        let limit = limit.min(self.limit);
        let declared = self
            .session
            .req_header()
            .headers
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<usize>().ok());
        if declared.is_some_and(|declared| declared > limit) {
            return Err(format!(
                "the request body is larger than the {limit} bytes a script may read here"
            ));
        }
        self.session.as_mut().enable_retry_buffering();
        let mut body = BytesMut::new();
        loop {
            let chunk = self
                .session
                .read_request_body()
                .await
                .map_err(|error| format!("the request body could not be read: {error}"))?;
            let Some(chunk) = chunk else {
                break;
            };
            if body.len() + chunk.len() > limit {
                return Err(format!(
                    "the request body is larger than the {limit} bytes a script may read here"
                ));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body.freeze())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_decode_for_scripts_and_encode_back() {
        assert_eq!(decoded_path("/a%20b/%C3%A9"), "/a b/é");
        assert_eq!(
            utf8_percent_encode("/new path/é?#", PATH).to_string(),
            "/new%20path/%C3%A9%3F%23"
        );
    }

    #[test]
    fn header_plans_touch_only_changed_fields() {
        let mut old = HeaderMap::new();
        old.insert("a", HeaderValue::from_static("1"));
        old.insert("b", HeaderValue::from_static("2"));
        let mut new = old.clone();
        new.remove("a");
        new.append("b", HeaderValue::from_static("3"));
        new.insert("c", HeaderValue::from_static("4"));
        let (removed, appended) = header_plan(&old, &new);
        assert_eq!(removed, ["a", "b"]);
        let appended: Vec<_> = appended
            .iter()
            .map(|(name, value)| (name.as_str(), value.to_str().unwrap()))
            .collect();
        assert_eq!(appended, [("b", "2"), ("b", "3"), ("c", "4")]);
    }
}
