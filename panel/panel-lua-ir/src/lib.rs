#![forbid(unsafe_code)]

//! A runtime snapshot's Lua for the Lua runtime (ADR 0039): the program its
//! scripts make, and the hooks its sites, routes and upstreams run, by their
//! IR identifiers. The gateway runs them on requests; the control plane runs
//! them to test scripts, with the same limits.

use panel_errors::{Diagnostic, ErrorCode, PanelError, Result};
use panel_ir::{LuaFallback, LuaHandler, LuaHandlers, LuaLogLevel, RouteAction, RuntimeSnapshot};
use panel_lua::{
    Handler, HandlerId, Limits, LogLevel, Permissions, Phase, Program, Settings, Source,
};
use std::{collections::HashMap, sync::Arc, time::Duration};

/// What a handler may take when its terms leave the limit to the runtime.
pub const DEFAULT_TIME: Duration = Duration::from_millis(100);
pub const DEFAULT_WORK: u64 = 10_000_000;
/// Runs longer than this are slow when the terms set no threshold.
pub const DEFAULT_SLOW: Duration = Duration::from_millis(10);
/// The memory of a VM when the configuration sets none.
pub const DEFAULT_MEMORY: usize = 64 << 20;

/// A handler as a request runs it.
#[derive(Clone, Debug)]
pub struct Hook {
    pub handler: Handler,
    pub fallback: LuaFallback,
    pub slow: Duration,
    pub debug: bool,
    /// The script's identifier in the snapshot.
    pub script: Arc<str>,
}

impl Hook {
    pub fn phase(&self) -> Phase {
        self.handler.phase
    }
}

/// The hooks a site or route runs, by phase.
#[derive(Clone, Debug, Default)]
pub struct Hooks {
    pub server_rewrite: Option<Hook>,
    pub rewrite: Option<Hook>,
    pub access: Option<Hook>,
    pub header_filter: Option<Hook>,
    pub body_filter: Option<Hook>,
    pub log: Option<Hook>,
}

impl Hooks {
    /// The hook of `phase`, if it runs one.
    pub fn get(&self, phase: Phase) -> Option<&Hook> {
        match phase {
            Phase::ServerRewrite => self.server_rewrite.as_ref(),
            Phase::Rewrite => self.rewrite.as_ref(),
            Phase::Access => self.access.as_ref(),
            Phase::HeaderFilter => self.header_filter.as_ref(),
            Phase::BodyFilter => self.body_filter.as_ref(),
            Phase::Log => self.log.as_ref(),
            _ => None,
        }
    }
}

/// Hooks by the IR identifier of what runs them.
#[derive(Debug, Default)]
pub struct HookIndex {
    pub sites: HashMap<String, Hooks>,
    pub routes: HashMap<String, Hooks>,
    /// Routes that answer with a script.
    pub contents: HashMap<String, Hook>,
    /// Upstream pools that choose their endpoints with a script.
    pub balancers: HashMap<String, Hook>,
}

/// A snapshot's scripts, compiled, with what starting their VMs takes.
#[derive(Debug)]
pub struct Compiled {
    pub program: Program,
    pub settings: Settings,
    pub index: HookIndex,
    /// `lua off`: the scripts compile but none runs.
    pub disabled: bool,
}

/// The runtime's level of an `ngx.log` level.
pub fn level(level: LuaLogLevel) -> LogLevel {
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

/// The runtime's terms for `handler` in `phase`, with the defaults filled in.
pub fn handler(id: HandlerId, handler: &LuaHandler, phase: Phase) -> Handler {
    Handler {
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
    }
}

struct Compiler<'a> {
    snapshot: &'a RuntimeSnapshot,
    builder: panel_lua::ProgramBuilder,
    handlers: HashMap<String, HandlerId>,
}

impl Compiler<'_> {
    fn hook(&mut self, terms: &LuaHandler, phase: Phase) -> Result<Hook> {
        let script = self.snapshot.lua.script(&terms.script_id).ok_or_else(|| {
            PanelError::validation_failed(format!(
                "a Lua handler names unknown script {}",
                terms.script_id
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
            handler: handler(id, terms, phase),
            fallback: terms.on_error,
            slow: match terms.slow_threshold_ms {
                0 => DEFAULT_SLOW,
                millis => Duration::from_millis(millis),
            },
            debug: terms.debug,
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

/// Compiles `snapshot`'s scripts for `vms` VMs; `None` when it has none.
/// Scripts that do not compile refuse the snapshot, with each error at the
/// line of the file it is in.
pub fn compile(snapshot: &RuntimeSnapshot, vms: usize) -> Result<Option<Compiled>> {
    if !panel_engine::uses_lua(snapshot) {
        return Ok(None);
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
    Ok(Some(Compiled {
        program,
        settings,
        index,
        disabled: snapshot.lua.disabled,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_domain::{PathPrefix, RevisionId, RouteId, SiteId, UpstreamPoolId};
    use panel_ir::{LuaProgram, LuaScript, RouteMatcher, RouteSpec, UpstreamPoolSpec};
    use panel_lua::{Connection, Exchange, NoHost, Outcome, Request, Runtime, SharedStore};

    fn script(id: &str, source: &str, module: Option<&str>) -> LuaScript {
        LuaScript {
            id: id.into(),
            file: id.into(),
            line: 1,
            source: source.into(),
            sha256: String::new(),
            module: module.map(Into::into),
        }
    }

    #[tokio::test]
    async fn snapshots_become_a_program_and_hooks_by_identifier() {
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        assert!(compile(&snapshot, 1).unwrap().is_none());
        snapshot.lua = LuaProgram {
            scripts: vec![
                script("main.conf:3", "ngx.say(require('greeting').text)", None),
                script(
                    "lua/greeting.lua",
                    "return { text = 'hi' }",
                    Some("greeting"),
                ),
                script("lua/pick.lua", "local n = 1", None),
            ],
            init: Some(LuaHandler::new("lua/pick.lua")),
            ..LuaProgram::default()
        };
        let mut content = LuaHandler::new("main.conf:3");
        content.time_limit_ms = 5;
        snapshot.routes.push(RouteSpec::new(
            RouteId::new("r-1").unwrap(),
            SiteId::new("s-1").unwrap(),
            10,
            RouteMatcher::PathPrefix {
                path: PathPrefix::new("/").unwrap(),
            },
            RouteAction::Lua { handler: content },
        ));
        let mut pool =
            UpstreamPoolSpec::new(UpstreamPoolId::new("p-1").unwrap(), "app", Vec::new());
        pool.balancer = Some(LuaHandler::new("lua/pick.lua"));
        snapshot.upstream_pools.push(pool);

        let compiled = compile(&snapshot, 2).unwrap().unwrap();
        assert_eq!(compiled.settings.vms, 2);
        assert_eq!(compiled.settings.memory, DEFAULT_MEMORY);
        let hook = compiled.index.contents["r-1"].clone();
        assert_eq!(hook.phase(), Phase::Content);
        assert_eq!(hook.handler.limits.time, Duration::from_millis(5));
        assert_eq!(hook.handler.limits.work, DEFAULT_WORK);
        assert_eq!(hook.slow, DEFAULT_SLOW);
        assert_eq!(compiled.index.balancers["p-1"].phase(), Phase::Balancer);

        let (runtime, _) = Runtime::start(
            &compiled.program,
            &compiled.settings,
            &SharedStore::default(),
        )
        .unwrap();
        let mut scripts = runtime.scripts(Exchange::new(Request::default(), Connection::default()));
        assert!(matches!(
            scripts.run(hook.handler, &mut NoHost).await,
            Outcome::Respond
        ));
        assert_eq!(scripts.exchange().response.body, b"hi\n");

        snapshot.lua.scripts[0].source = "ngx.say(".into();
        let refused = compile(&snapshot, 1).unwrap_err();
        let message = &refused.diagnostics[0].message;
        assert!(message.starts_with("main.conf:3:1:"), "{message}");
    }
}
