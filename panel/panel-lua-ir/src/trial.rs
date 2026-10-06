//! A request tried on a snapshot's Lua handlers without proxying it (ADR
//! 0039): the handlers the gateway would run for it, in the gateway's
//! order, each with how it ended, what it logged and how long it took, and
//! the request and response as they were left.

use crate::{compile, compile_script, Compiled, Hook, HookIndex, Hooks, Variable, DEFAULT_MEMORY};
use async_trait::async_trait;
use bytes::Bytes;
use http::{header::CONTENT_LENGTH, HeaderMap, HeaderName, HeaderValue, Version};
use panel_errors::{PanelError, Result};
use panel_ir::{
    template::{parse_template, RequestVariable, TemplatePart},
    LuaFallback, LuaHandler, RouteAction, RuntimeSnapshot,
};
use panel_lua::{
    Chunk, Connection, Exchange, FailureKind, Host, LogEntry, Outcome, Peer, Phase, Program,
    Request, Runtime, Scripts, Settings, SharedStore, Source,
};
use panel_routing::{path, Router, SimulatedRequest};
use percent_encoding::percent_decode_str;
use std::{
    net::{IpAddr, SocketAddr},
    time::{Duration, Instant, SystemTime},
};

/// Rewrites that choose the route again before the gateway stops, as it does.
/// URI changes a request may make, by jumps and internal redirects together,
/// as nginx allows.
const MOST_URI_CHANGES: usize = 10;

/// A request to try.
#[derive(Clone, Debug)]
pub struct TrialRequest {
    pub method: String,
    /// The host it names, without a port.
    pub host: String,
    /// The path and query as sent, such as `/api?x=1`.
    pub target: String,
    /// Field lines in order.
    pub headers: Vec<(String, String)>,
    pub body: Option<Bytes>,
    /// The client, after trusted proxies.
    pub client: Option<IpAddr>,
    pub tls: bool,
    /// The listener it arrives on, which may serve only some sites.
    pub listener: Option<String>,
    pub request_id: String,
}

/// The upstream's answer to a proxied request, which the response's
/// filters and the log handler see when no script answers.
#[derive(Clone, Debug)]
pub struct TrialResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
}

impl Default for TrialResponse {
    fn default() -> Self {
        Self {
            status: 200,
            headers: Vec::new(),
            body: Bytes::new(),
        }
    }
}

/// What the trial runs.
#[derive(Clone, Debug)]
pub enum TrialScript {
    /// The handlers of the snapshot the request reaches.
    Configured,
    /// One script, in place of the snapshot's handler of `phase`.
    Script {
        source: Source,
        terms: LuaHandler,
        phase: Phase,
    },
}

/// How a run ended.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunOutcome {
    /// The request goes on.
    Continue,
    /// The handler answered.
    Respond,
    /// The connection closes without an answer.
    Abort,
    /// An error, a timeout or a limit; the handler's fallback applies.
    Failed { kind: FailureKind, message: String },
}

/// One handler's run.
#[derive(Clone, Debug)]
pub struct TrialRun {
    pub phase: Phase,
    /// The script's identifier in the snapshot.
    pub script: String,
    pub outcome: RunOutcome,
    pub duration: Duration,
    pub logs: Vec<LogEntry>,
}

/// What became of the request.
#[derive(Clone, Debug, Default)]
pub struct Trial {
    /// The IR identifiers of the site and route it reached.
    pub site: Option<String>,
    pub route: Option<String>,
    /// What `init_by_lua` and `init_worker_by_lua` logged.
    pub init_logs: Vec<LogEntry>,
    pub runs: Vec<TrialRun>,
    /// The request as the handlers left it.
    pub request: Option<Request>,
    /// The response the client gets, unless the connection closed.
    pub response: Option<(u16, HeaderMap, Bytes)>,
    /// Whether a handler closed the connection.
    pub aborted: bool,
    /// The endpoint a balancer chose.
    pub peer: Option<Peer>,
}

/// Gives scripts the body the request was described with.
struct TrialHost(Option<Bytes>);

#[async_trait]
impl Host for TrialHost {
    async fn read_body(&mut self, limit: usize) -> std::result::Result<Bytes, String> {
        let body = self.0.clone().unwrap_or_default();
        if body.len() > limit {
            return Err(format!("the request body is larger than {limit} bytes"));
        }
        Ok(body)
    }

    /// The whole body as one piece: a test has it all at once.
    async fn read_body_chunk(&mut self) -> std::result::Result<Option<Bytes>, String> {
        Ok(self.0.take().filter(|body| !body.is_empty()))
    }
}

fn headers(lines: &[(String, String)]) -> Result<HeaderMap> {
    let mut map = HeaderMap::new();
    for (name, value) in lines {
        let name = HeaderName::from_bytes(name.trim().as_bytes()).map_err(|_| {
            PanelError::invalid_argument(format!("{name:?} is not a header field name"))
        })?;
        let value = HeaderValue::from_str(value).map_err(|_| {
            PanelError::invalid_argument(format!("the value of {name} is not a field value"))
        })?;
        map.append(name, value);
    }
    Ok(map)
}

fn routing_request(exchange: &Exchange, host: &str) -> Option<SimulatedRequest> {
    let request = &exchange.request;
    Some(SimulatedRequest {
        method: request.method.clone(),
        host: host.to_owned(),
        path: path::normalize(&request.uri)?.into_owned(),
        query: request.args.clone(),
        headers: request
            .headers
            .iter()
            .filter_map(|(name, value)| {
                Some((name.as_str().to_owned(), value.to_str().ok()?.to_owned()))
            })
            .collect(),
        client: exchange.connection.client.map(|client| client.ip()),
    })
}

/// How a run ended, and what the request does next.
enum Next {
    Go,
    Answer,
    Close,
    /// `ngx.exec`: the request starts over with its new URI.
    Redirect,
}

struct Runner<'a> {
    scripts: Scripts,
    host: TrialHost,
    trial: &'a mut Trial,
}

/// `template` filled in from the request as the handlers left it.
fn fill(template: &str, exchange: &Exchange) -> String {
    let Ok(parts) = parse_template(template) else {
        return String::new();
    };
    let request = &exchange.request;
    let header = |name: &str| {
        request
            .headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    let mut out = String::new();
    for part in parts {
        let TemplatePart::Variable(variable) = part else {
            if let TemplatePart::Text(text) = part {
                out.push_str(&text);
            }
            continue;
        };
        let value = match variable {
            RequestVariable::Host => Some(exchange.connection.server_name.clone()),
            RequestVariable::Uri => Some(request.uri.clone()),
            RequestVariable::Method => Some(request.method.clone()),
            RequestVariable::Scheme => Some(
                if exchange.connection.tls {
                    "https"
                } else {
                    "http"
                }
                .to_owned(),
            ),
            RequestVariable::ClientIp => exchange
                .connection
                .client
                .map(|client| client.ip().to_string()),
            RequestVariable::RequestId => Some(exchange.connection.request_id.clone()),
            RequestVariable::Header(name) => header(&name),
            RequestVariable::Cookie(name) => header("cookie").and_then(|line| {
                line.split(';').find_map(|pair| {
                    let (key, value) = pair.trim().split_once('=')?;
                    (key == name).then(|| value.to_owned())
                })
            }),
            RequestVariable::Lua(name) => exchange.variables.get(&name).cloned(),
            _ => None,
        };
        out.push_str(value.as_deref().unwrap_or_default());
    }
    out
}

impl Runner<'_> {
    async fn run(&mut self, hook: &Hook) -> Next {
        self.run_as(hook, None).await
    }

    /// Sets `variables` in order, running the handlers of those a script
    /// sets.
    async fn variables(&mut self, variables: &[Variable]) -> Next {
        for variable in variables {
            let Some(hook) = &variable.hook else {
                self.scripts
                    .exchange()
                    .variables
                    .insert(variable.name.to_string(), variable.value.to_string());
                continue;
            };
            let args = {
                let exchange = self.scripts.exchange();
                variable
                    .args
                    .iter()
                    .map(|arg| fill(arg, &exchange))
                    .collect()
            };
            match self.run_as(hook, Some((&variable.name, args))).await {
                Next::Go => {}
                next => return next,
            }
        }
        Next::Go
    }

    async fn run_as(&mut self, hook: &Hook, set: Option<(&str, Vec<String>)>) -> Next {
        let started = Instant::now();
        let outcome = match set {
            Some((name, args)) => {
                self.scripts
                    .set(hook.handler, &mut self.host, name, args)
                    .await
            }
            None => self.scripts.run(hook.handler, &mut self.host).await,
        };
        let duration = started.elapsed();
        let logs = std::mem::take(&mut self.scripts.exchange().logs);
        let (outcome, next) = match outcome {
            Outcome::Continue if self.scripts.exchange().redirected() => {
                (RunOutcome::Continue, Next::Redirect)
            }
            Outcome::Continue => (RunOutcome::Continue, Next::Go),
            Outcome::Respond => (RunOutcome::Respond, Next::Answer),
            Outcome::Abort => (RunOutcome::Abort, Next::Close),
            Outcome::Failed(failure) => {
                let next = match hook.fallback {
                    LuaFallback::Continue => Next::Go,
                    LuaFallback::Status { status } => {
                        self.answer(status);
                        Next::Answer
                    }
                    _ => {
                        self.answer(if hook.phase() == Phase::Balancer {
                            502
                        } else {
                            500
                        });
                        Next::Answer
                    }
                };
                (
                    RunOutcome::Failed {
                        kind: failure.kind,
                        message: failure.message,
                    },
                    next,
                )
            }
        };
        self.trial.runs.push(TrialRun {
            phase: hook.phase(),
            script: hook.script.to_string(),
            outcome,
            duration,
            logs,
        });
        next
    }

    /// The gateway answers with `status` and an empty body.
    fn answer(&mut self, status: u16) {
        let mut exchange = self.scripts.exchange();
        exchange.response.status = status;
        exchange.response.headers.clear();
        exchange.response.body.clear();
    }

    /// Answers 500, as the gateway does once a request has changed its URI
    /// more often than it may.
    async fn cycle(&mut self, hooks: Option<&Hooks>) {
        self.answer(500);
        self.scripts.exchange().response.body = b"rewrite or internal redirection cycle".to_vec();
        self.respond(hooks).await;
        self.log(hooks).await;
    }

    /// Lets the request start over after `ngx.exec`, or answers 500 once it
    /// has changed its URI too often. Returns whether it starts over.
    async fn restart(&mut self, changes_left: &mut usize, hooks: Option<&Hooks>) -> bool {
        if *changes_left == 0 {
            self.cycle(hooks).await;
            return false;
        }
        *changes_left -= 1;
        true
    }

    /// Sets the upstream's response as the one the filters see.
    fn upstream(&mut self, response: &TrialResponse) -> Result<()> {
        let headers = headers(&response.headers)?;
        let mut exchange = self.scripts.exchange();
        exchange.response.status = response.status;
        exchange.response.headers = headers;
        exchange.response.body = response.body.to_vec();
        Ok(())
    }

    /// The response's header and body filters and the log handler of `hooks`.
    async fn respond(&mut self, hooks: Option<&Hooks>) {
        if let Some(hook) = hooks.and_then(|hooks| hooks.header_filter.as_ref()) {
            self.run(hook).await;
        }
        if let Some(hook) = hooks.and_then(|hooks| hooks.body_filter.as_ref()) {
            {
                let mut exchange = self.scripts.exchange();
                let body = std::mem::take(&mut exchange.response.body);
                exchange.chunk = Chunk {
                    data: Bytes::from(body),
                    eof: true,
                };
            }
            self.run(hook).await;
            let mut exchange = self.scripts.exchange();
            exchange.response.body = exchange.chunk.data.to_vec();
        }
        let exchange = self.scripts.exchange();
        let mut headers = exchange.response.headers.clone();
        headers.remove(CONTENT_LENGTH);
        self.trial.response = Some((
            match exchange.response.status {
                0 => 200,
                status => status,
            },
            headers,
            Bytes::from(exchange.response.body.clone()),
        ));
    }

    async fn log(&mut self, hooks: Option<&Hooks>) {
        if let Some(hook) = hooks.and_then(|hooks| hooks.log.as_ref()) {
            self.run(hook).await;
        }
        self.trial.request = Some(self.scripts.exchange().request.clone());
        self.trial
            .peer
            .clone_from(&self.scripts.exchange().balancer.peer);
    }
}

/// Runs what `script` names for `request` with a fresh VM and shared
/// dictionaries; `upstream` is the response of a request the route proxies.
pub async fn try_request(
    snapshot: &RuntimeSnapshot,
    script: TrialScript,
    request: TrialRequest,
    upstream: TrialResponse,
) -> Result<Trial> {
    let (compiled, only) = match script {
        TrialScript::Configured => match compile(snapshot, 1)? {
            Some(compiled) => (compiled, None),
            None => (
                Compiled {
                    program: Program::default(),
                    settings: Settings {
                        vms: 1,
                        memory: DEFAULT_MEMORY,
                    },
                    index: HookIndex::default(),
                    disabled: false,
                },
                None,
            ),
        },
        TrialScript::Script {
            source,
            terms,
            phase,
        } => {
            let (compiled, hook) = compile_script(snapshot, &source, &terms, phase)?;
            (compiled, Some(hook))
        }
    };
    let (runtime, init_logs) = Runtime::start(
        &compiled.program,
        &compiled.settings,
        &SharedStore::default(),
    )
    .map_err(|failure| {
        PanelError::validation_failed(format!(
            "init_by_lua failed ({}): {}",
            failure.kind.name(),
            failure.message
        ))
    })?;
    runtime.isolate();
    let (raw_path, query) = match request.target.split_once('?') {
        Some((path, query)) => (path, Some(query.to_owned())),
        None => (request.target.as_str(), None),
    };
    if !raw_path.starts_with('/') {
        return Err(PanelError::invalid_argument(
            "the target must be an absolute path, such as /api/items?tag=new",
        ));
    }
    let exchange = Exchange::new(
        Request {
            method: request.method.clone(),
            uri: percent_decode_str(raw_path)
                .decode_utf8_lossy()
                .into_owned(),
            request_uri: request.target.clone(),
            args: query,
            version: Version::HTTP_11,
            headers: headers(&request.headers)?,
            body: None,
        },
        Connection {
            client: request.client.map(|ip| SocketAddr::new(ip, 0)),
            server: None,
            tls: request.tls,
            server_name: request.host.clone(),
            request_id: request.request_id.clone(),
            started: SystemTime::now(),
        },
    );
    let mut trial = Trial {
        init_logs,
        ..Trial::default()
    };
    let mut runner = Runner {
        scripts: runtime.scripts(exchange),
        host: TrialHost(request.body.clone()),
        trial: &mut trial,
    };
    if let Some(hook) = only {
        if matches!(
            hook.phase(),
            Phase::HeaderFilter | Phase::BodyFilter | Phase::Log
        ) {
            runner.upstream(&upstream)?;
        }
        let hooks = Hooks {
            header_filter: (hook.phase() == Phase::HeaderFilter).then(|| hook.clone()),
            body_filter: (hook.phase() == Phase::BodyFilter).then(|| hook.clone()),
            log: (hook.phase() == Phase::Log).then(|| hook.clone()),
            ..Hooks::default()
        };
        match hook.phase() {
            Phase::HeaderFilter | Phase::BodyFilter => runner.respond(Some(&hooks)).await,
            Phase::Log => {}
            _ => match runner.run(&hook).await {
                Next::Answer => runner.respond(None).await,
                Next::Close => runner.trial.aborted = true,
                Next::Go | Next::Redirect => {}
            },
        }
        runner.log(Some(&hooks)).await;
        return Ok(trial);
    }
    if compiled.disabled {
        return Ok(trial);
    }
    pipeline(snapshot, &compiled.index, &mut runner, &request, &upstream).await?;
    Ok(trial)
}

/// The handlers the gateway runs for the request, in its order.
async fn pipeline(
    snapshot: &RuntimeSnapshot,
    index: &HookIndex,
    runner: &mut Runner<'_>,
    request: &TrialRequest,
    upstream: &TrialResponse,
) -> Result<()> {
    let router = Router::compile(snapshot)?;
    let host = request.host.trim_end_matches('.').to_ascii_lowercase();
    let listener = request.listener.as_deref();
    let site = router
        .lookup(&host)
        .filter(|entry| listener.is_none_or(|listener| router.site(entry.site).serves(listener)))
        .map(|entry| entry.site)
        .or_else(|| listener.and_then(|listener| router.default_site(listener)));
    let Some(site) = site.map(|site| router.site(site)) else {
        return Ok(());
    };
    runner.trial.site = Some(site.id.as_str().to_owned());
    let site_hooks = index.sites.get(site.id.as_str());
    let mut changes_left = MOST_URI_CHANGES;
    'request: loop {
        // A named location starts at its own rewrite phase.
        let mut named = runner.scripts.exchange().take_named();
        let variables = site_hooks
            .filter(|_| named.is_none())
            .map_or(&[][..], |hooks| &hooks.variables);
        match runner.variables(variables).await {
            Next::Go => {}
            Next::Answer => {
                runner.respond(site_hooks).await;
                runner.log(site_hooks).await;
                return Ok(());
            }
            Next::Close | Next::Redirect => {
                runner.trial.aborted = true;
                runner.log(site_hooks).await;
                return Ok(());
            }
        }
        if let Some(hook) = site_hooks
            .filter(|_| named.is_none())
            .and_then(|hooks| hooks.server_rewrite.as_ref())
        {
            match runner.run(hook).await {
                Next::Go => {}
                Next::Redirect => {
                    if runner.restart(&mut changes_left, site_hooks).await {
                        continue 'request;
                    }
                    return Ok(());
                }
                Next::Answer => {
                    runner.respond(site_hooks).await;
                    runner.log(site_hooks).await;
                    return Ok(());
                }
                Next::Close => {
                    runner.trial.aborted = true;
                    runner.log(site_hooks).await;
                    return Ok(());
                }
            }
        }
        let mut route;
        loop {
            let simulated =
                routing_request(&runner.scripts.exchange(), &host).ok_or_else(|| {
                    PanelError::invalid_argument("a script set a path that is not absolute")
                })?;
            route = match named.take() {
                Some(name) => {
                    let Some(position) = site.named(&name) else {
                        runner.answer(500);
                        runner.scripts.exchange().response.body =
                            format!("could not find named location \"@{name}\"").into_bytes();
                        runner.respond(site_hooks).await;
                        runner.log(site_hooks).await;
                        return Ok(());
                    };
                    Some(site.route(position))
                }
                None => site.select(&simulated).map(|position| site.route(position)),
            };
            runner.trial.route = route.map(|route| route.id.as_str().to_owned());
            let hooks = route.and_then(|route| index.routes.get(route.id.as_str()));
            let variables = hooks.map_or(&[][..], |hooks| &hooks.variables);
            match runner.variables(variables).await {
                Next::Go => {}
                Next::Answer => {
                    runner.respond(hooks).await;
                    runner.log(hooks).await;
                    return Ok(());
                }
                Next::Close | Next::Redirect => {
                    runner.trial.aborted = true;
                    runner.log(hooks).await;
                    return Ok(());
                }
            }
            let Some(hook) = hooks.and_then(|hooks| hooks.rewrite.as_ref()) else {
                break;
            };
            runner.scripts.exchange().clear_changes();
            match runner.run(hook).await {
                Next::Go if runner.scripts.exchange().changes().jump => {
                    if changes_left == 0 {
                        runner.cycle(hooks).await;
                        return Ok(());
                    }
                    changes_left -= 1;
                }
                Next::Go => break,
                Next::Redirect => {
                    if runner.restart(&mut changes_left, hooks).await {
                        continue 'request;
                    }
                    return Ok(());
                }
                Next::Answer => {
                    runner.respond(hooks).await;
                    runner.log(hooks).await;
                    return Ok(());
                }
                Next::Close => {
                    runner.trial.aborted = true;
                    runner.log(hooks).await;
                    return Ok(());
                }
            }
        }
        let Some(route) = route else {
            runner.log(site_hooks).await;
            return Ok(());
        };
        let hooks = index.routes.get(route.id.as_str());
        if let Some(hook) = hooks.and_then(|hooks| hooks.access.as_ref()) {
            match runner.run(hook).await {
                Next::Go => {}
                Next::Redirect => {
                    if runner.restart(&mut changes_left, hooks).await {
                        continue 'request;
                    }
                    return Ok(());
                }
                Next::Answer => {
                    runner.respond(hooks).await;
                    runner.log(hooks).await;
                    return Ok(());
                }
                Next::Close => {
                    runner.trial.aborted = true;
                    runner.log(hooks).await;
                    return Ok(());
                }
            }
        }
        if let Some(hook) = hooks.and_then(|hooks| hooks.precontent.as_ref()) {
            match runner.run(hook).await {
                Next::Go => {}
                Next::Redirect => {
                    if runner.restart(&mut changes_left, hooks).await {
                        continue 'request;
                    }
                    return Ok(());
                }
                Next::Answer => {
                    runner.respond(hooks).await;
                    runner.log(hooks).await;
                    return Ok(());
                }
                Next::Close => {
                    runner.trial.aborted = true;
                    runner.log(hooks).await;
                    return Ok(());
                }
            }
        }
        match &snapshot.routes[route.spec].action {
            RouteAction::Lua { .. } => {
                if let Some(hook) = index.contents.get(route.id.as_str()) {
                    match runner.run(hook).await {
                        Next::Close => {
                            runner.trial.aborted = true;
                            runner.log(hooks).await;
                            return Ok(());
                        }
                        Next::Redirect => {
                            if runner.restart(&mut changes_left, hooks).await {
                                continue 'request;
                            }
                            return Ok(());
                        }
                        Next::Go | Next::Answer => {}
                    }
                }
            }
            RouteAction::Proxy { upstream_pool_id } => {
                if let Some(hook) = index.balancers.get(upstream_pool_id.as_str()) {
                    {
                        let mut exchange = runner.scripts.exchange();
                        exchange.balancer.upstream = upstream_pool_id.as_str().to_owned();
                    }
                    match runner.run(hook).await {
                        Next::Go | Next::Redirect => runner.upstream(upstream)?,
                        Next::Answer => {}
                        Next::Close => {
                            runner.trial.aborted = true;
                            runner.log(hooks).await;
                            return Ok(());
                        }
                    }
                } else {
                    runner.upstream(upstream)?;
                }
            }
            _ => runner.upstream(upstream)?,
        }
        runner.respond(hooks).await;
        runner.log(hooks).await;
        return Ok(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_domain::{NormalizedHost, PathPrefix, RevisionId, RouteId, SiteId};
    use panel_ir::{
        DomainSpec, LuaHandlers, LuaPermissions, LuaProgram, LuaScript, LuaVariable, RouteMatcher,
        RouteSpec, SiteSpec,
    };

    fn script(id: &str, source: &str) -> LuaScript {
        LuaScript {
            id: id.into(),
            file: "main.conf".into(),
            line: 1,
            source: source.into(),
            sha256: String::new(),
            module: None,
        }
    }

    fn snapshot() -> RuntimeSnapshot {
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(3));
        snapshot.lua = LuaProgram {
            scripts: vec![
                script(
                    "access",
                    "if ngx.var.http_x_key ~= 'k' then return ngx.exit(403) end",
                ),
                script("content", "ngx.say('hello ', ngx.var.arg_name or 'world')"),
                script("header", "ngx.header['X-Seen'] = '1'"),
                script("body", "ngx.arg[1] = string.upper(ngx.arg[1])"),
                script("log", "ngx.log(ngx.NOTICE, 'done')"),
            ],
            ..LuaProgram::default()
        };
        let site = SiteSpec::new(
            SiteId::new("shop").unwrap(),
            "shop",
            vec![DomainSpec::new(
                NormalizedHost::new("shop.example").unwrap(),
            )],
        );
        snapshot.sites.push(site);
        let mut route = RouteSpec::new(
            RouteId::new("hello").unwrap(),
            SiteId::new("shop").unwrap(),
            10,
            RouteMatcher::PathPrefix {
                path: PathPrefix::new("/").unwrap(),
            },
            RouteAction::Lua {
                handler: LuaHandler::new("content"),
            },
        );
        let mut body = LuaHandler::new("body");
        body.allow = LuaPermissions {
            body: true,
            ..LuaPermissions::default()
        };
        route.lua = LuaHandlers {
            access: Some(LuaHandler::new("access")),
            header_filter: Some(LuaHandler::new("header")),
            body_filter: Some(body),
            log: Some(LuaHandler::new("log")),
            ..LuaHandlers::default()
        };
        snapshot.routes.push(route);
        snapshot
    }

    fn request(target: &str, headers: &[(&str, &str)]) -> TrialRequest {
        TrialRequest {
            method: "GET".into(),
            host: "shop.example".into(),
            target: target.into(),
            headers: headers
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
            body: None,
            client: None,
            tls: false,
            listener: None,
            request_id: "test".into(),
        }
    }

    fn phases(trial: &Trial) -> Vec<(Phase, RunOutcome)> {
        trial
            .runs
            .iter()
            .map(|run| (run.phase, run.outcome.clone()))
            .collect()
    }

    #[tokio::test]
    async fn requests_run_the_handlers_they_reach_in_the_gateways_order() {
        let snapshot = snapshot();
        let trial = try_request(
            &snapshot,
            TrialScript::Configured,
            request("/?name=lua", &[("x-key", "k")]),
            TrialResponse::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            (trial.site.as_deref(), trial.route.as_deref()),
            (Some("shop"), Some("hello"))
        );
        assert_eq!(
            phases(&trial),
            [
                (Phase::Access, RunOutcome::Continue),
                (Phase::Content, RunOutcome::Respond),
                (Phase::HeaderFilter, RunOutcome::Continue),
                (Phase::BodyFilter, RunOutcome::Continue),
                (Phase::Log, RunOutcome::Continue),
            ]
        );
        let (status, headers, body) = trial.response.unwrap();
        assert_eq!((status, &body[..]), (200, &b"HELLO LUA\n"[..]));
        assert_eq!(headers["x-seen"], "1");
        assert_eq!(trial.runs[4].logs[0].message, "main.conf:1: done");

        let refused = try_request(
            &snapshot,
            TrialScript::Configured,
            request("/", &[]),
            TrialResponse::default(),
        )
        .await
        .unwrap();
        assert_eq!(refused.runs[0].outcome, RunOutcome::Respond);
        assert_eq!(refused.response.unwrap().0, 403);
        assert_eq!(
            refused.runs.len(),
            4,
            "access, then both filters and the log"
        );
    }

    #[tokio::test]
    async fn internal_redirects_start_the_request_over_as_the_gateway_does() {
        let mut snapshot = snapshot();
        snapshot.lua.scripts.extend([
            script("exec", "return ngx.exec('/', { name = 'again' })"),
            script("loop", "return ngx.exec('/loop')"),
        ]);
        for (id, path, handler) in [("exec", "/exec", "exec"), ("loop", "/loop", "loop")] {
            snapshot.routes.push(RouteSpec::new(
                RouteId::new(id).unwrap(),
                SiteId::new("shop").unwrap(),
                5,
                RouteMatcher::PathPrefix {
                    path: PathPrefix::new(path).unwrap(),
                },
                RouteAction::Lua {
                    handler: LuaHandler::new(handler),
                },
            ));
        }
        let trial = try_request(
            &snapshot,
            TrialScript::Configured,
            request("/exec", &[("x-key", "k")]),
            TrialResponse::default(),
        )
        .await
        .unwrap();
        assert_eq!(trial.route.as_deref(), Some("hello"));
        assert_eq!(
            phases(&trial)[..3],
            [
                (Phase::Content, RunOutcome::Continue),
                (Phase::Access, RunOutcome::Continue),
                (Phase::Content, RunOutcome::Respond),
            ]
        );
        let (status, _, body) = trial.response.unwrap();
        assert_eq!((status, &body[..]), (200, &b"HELLO AGAIN\n"[..]));

        let cycling = try_request(
            &snapshot,
            TrialScript::Configured,
            request("/loop", &[]),
            TrialResponse::default(),
        )
        .await
        .unwrap();
        assert_eq!(cycling.runs.len(), 11, "the first run and ten redirects");
        assert_eq!(cycling.response.unwrap().0, 500);
    }

    #[tokio::test]
    async fn precontent_handlers_run_between_access_and_the_action() {
        let mut snapshot = snapshot();
        snapshot
            .lua
            .scripts
            .push(script("pre", "ngx.req.set_header('X-Pre', '1')"));
        snapshot.routes[0].lua.precontent = Some(LuaHandler::new("pre"));
        let trial = try_request(
            &snapshot,
            TrialScript::Configured,
            request("/", &[("x-key", "k")]),
            TrialResponse::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            phases(&trial)[..3],
            [
                (Phase::Access, RunOutcome::Continue),
                (Phase::Precontent, RunOutcome::Continue),
                (Phase::Content, RunOutcome::Respond),
            ]
        );
        assert_eq!(trial.request.unwrap().headers["x-pre"], "1");
    }

    #[tokio::test]
    async fn variables_are_set_before_the_rewrite_handlers_of_their_block() {
        let mut snapshot = snapshot();
        snapshot.lua.scripts.extend([
            script("tenant", "return ngx.arg[1] .. ':' .. ngx.var.base"),
            script("pre", "ngx.req.set_header('X-Tenant', ngx.var.tenant)"),
        ]);
        snapshot.sites[0].lua.variables = vec![LuaVariable {
            name: "base".into(),
            value: "b".into(),
            handler: None,
            args: Vec::new(),
        }];
        snapshot.routes[0].lua.variables = vec![LuaVariable {
            name: "tenant".into(),
            value: String::new(),
            handler: Some(LuaHandler::new("tenant")),
            args: vec!["$http_x_key".into()],
        }];
        snapshot.routes[0].lua.precontent = Some(LuaHandler::new("pre"));
        let trial = try_request(
            &snapshot,
            TrialScript::Configured,
            request("/", &[("x-key", "k")]),
            TrialResponse::default(),
        )
        .await
        .unwrap();
        assert_eq!(
            phases(&trial)[..3],
            [
                (Phase::Set, RunOutcome::Continue),
                (Phase::Access, RunOutcome::Continue),
                (Phase::Precontent, RunOutcome::Continue),
            ]
        );
        assert_eq!(trial.request.unwrap().headers["x-tenant"], "k:b");
    }

    #[tokio::test]
    async fn exec_to_a_named_location_skips_the_server_rewrite() {
        let mut snapshot = snapshot();
        snapshot.lua.scripts.extend([
            script("named", "ngx.exec('@later')"),
            script("later", "ngx.say('later ', ngx.var.uri)"),
            script("server", "ngx.req.set_header('X-Server', '1')"),
        ]);
        snapshot.sites[0].lua.server_rewrite = Some(LuaHandler::new("server"));
        snapshot.routes[0].action = RouteAction::Lua {
            handler: LuaHandler::new("named"),
        };
        snapshot.routes[0].lua.access = None;
        snapshot.routes.push(RouteSpec::new(
            RouteId::new("later").unwrap(),
            SiteId::new("shop").unwrap(),
            20,
            RouteMatcher::Named {
                name: "later".into(),
            },
            RouteAction::Lua {
                handler: LuaHandler::new("later"),
            },
        ));
        let trial = try_request(
            &snapshot,
            TrialScript::Configured,
            request("/x", &[]),
            TrialResponse::default(),
        )
        .await
        .unwrap();
        assert_eq!(trial.route.as_deref(), Some("later"));
        let ran: Vec<_> = trial.runs.iter().map(|run| run.script.as_str()).collect();
        assert_eq!(ran[..3], ["server", "named", "later"]);
        let (_, _, body) = trial.response.unwrap();
        assert_eq!(&body[..], b"later /x\n");
    }

    #[tokio::test]
    async fn one_script_runs_in_place_of_the_phases_handler() {
        let trial = try_request(
            &snapshot(),
            TrialScript::Script {
                source: Source::new("editor", "ngx.req.set_header('X-A', ngx.var.uri)", 1),
                terms: LuaHandler::new("editor"),
                phase: Phase::Rewrite,
            },
            request("/caf%C3%A9", &[]),
            TrialResponse::default(),
        )
        .await
        .unwrap();
        assert_eq!(phases(&trial), [(Phase::Rewrite, RunOutcome::Continue)]);
        assert_eq!(trial.request.unwrap().headers["x-a"], "/café");
        let nowhere = try_request(
            &snapshot(),
            TrialScript::Configured,
            TrialRequest {
                host: "other.example".into(),
                ..request("/", &[])
            },
            TrialResponse::default(),
        )
        .await
        .unwrap();
        assert!(nowhere.site.is_none() && nowhere.runs.is_empty());
    }
}
