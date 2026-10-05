//! Lua directives (ADR 0039): the handlers of each phase, the terms they
//! run on, what runs once in each VM, and the shared dictionaries.

use super::{Lowerer, Origin};
use crate::{codes, values};
use panel_config_model::{
    Action, LuaCode, LuaFallback, LuaLogLevel, LuaPermissions, LuaScope, LuaSharedDict,
    LUA_DIRECTORY,
};
use panel_dsl::Directive;

/// Directives that set a handler or a term of `http`, a server or a route.
pub(super) const SCOPE: &[&str] = &[
    "server_rewrite_by_lua_block",
    "server_rewrite_by_lua_file",
    "rewrite_by_lua_block",
    "rewrite_by_lua_file",
    "access_by_lua_block",
    "access_by_lua_file",
    "header_filter_by_lua_block",
    "header_filter_by_lua_file",
    "body_filter_by_lua_block",
    "body_filter_by_lua_file",
    "log_by_lua_block",
    "log_by_lua_file",
    "lua_time_limit",
    "lua_work_limit",
    "lua_allow",
    "lua_on_error",
    "lua_log_level",
    "lua_slow_threshold",
    "lua_debug",
    "lua_code_cache",
];

/// The directives that set the same handler or term, in the order they are
/// explained.
pub(crate) const GROUPS: &[&[&str]] = &[
    &["server_rewrite_by_lua_block", "server_rewrite_by_lua_file"],
    &["rewrite_by_lua_block", "rewrite_by_lua_file"],
    &["access_by_lua_block", "access_by_lua_file"],
    &["header_filter_by_lua_block", "header_filter_by_lua_file"],
    &["body_filter_by_lua_block", "body_filter_by_lua_file"],
    &["log_by_lua_block", "log_by_lua_file"],
    &["lua_time_limit"],
    &["lua_work_limit"],
    &["lua_allow"],
    &["lua_on_error"],
    &["lua_log_level"],
    &["lua_slow_threshold"],
    &["lua_debug"],
];

/// What each term is when no block around a handler writes it.
pub(crate) const DEFAULTS: &[(&str, &str)] = &[
    ("lua_time_limit", "100ms"),
    ("lua_work_limit", "10000000"),
    ("lua_allow", "none"),
    ("lua_on_error", "fail"),
    ("lua_log_level", "notice"),
    ("lua_slow_threshold", "10ms"),
    ("lua_debug", "off"),
];

/// The directives that set the terms handlers run on.
pub(crate) const TERMS: &[&str] = &[
    "lua_time_limit",
    "lua_work_limit",
    "lua_allow",
    "lua_on_error",
    "lua_log_level",
    "lua_slow_threshold",
    "lua_debug",
];

/// Whether a Lua handler runs for the requests of a block with `scope`
/// and `action`.
pub(crate) fn runs(scope: &LuaScope, action: &Action) -> bool {
    scope.handlers().next().is_some() || matches!(action, Action::Lua { .. })
}

/// Directives only `http` takes.
pub(super) const HTTP: &[&str] = &[
    "lua",
    "lua_shared_dict",
    "lua_memory_limit",
    "init_by_lua_block",
    "init_by_lua_file",
    "init_worker_by_lua_block",
    "init_worker_by_lua_file",
];

/// The phase a handler directive runs in.
fn phase(name: &str) -> Option<&'static str> {
    let stem = name
        .strip_suffix("_by_lua_block")
        .or_else(|| name.strip_suffix("_by_lua_file"))?;
    [
        "server_rewrite",
        "rewrite",
        "access",
        "content",
        "balancer",
        "header_filter",
        "body_filter",
        "log",
        "init",
        "init_worker",
    ]
    .into_iter()
    .find(|phase| *phase == stem)
}

const LOG_LEVELS: [(&str, LuaLogLevel); 9] = [
    ("stderr", LuaLogLevel::Stderr),
    ("emerg", LuaLogLevel::Emerg),
    ("alert", LuaLogLevel::Alert),
    ("crit", LuaLogLevel::Crit),
    ("error", LuaLogLevel::Error),
    ("warn", LuaLogLevel::Warn),
    ("notice", LuaLogLevel::Notice),
    ("info", LuaLogLevel::Info),
    ("debug", LuaLogLevel::Debug),
];

fn log_level_name(level: LuaLogLevel) -> &'static str {
    LOG_LEVELS
        .iter()
        .find(|(_, known)| *known == level)
        .map_or("notice", |(name, _)| name)
}

impl<'a> Lowerer<'a> {
    /// The code of a `*_by_lua_block` or `*_by_lua_file` directive.
    pub(super) fn lua_code(&mut self, file: &str, directive: &Directive) -> Option<LuaCode> {
        let name = directive.name.value.as_str();
        if name.ends_with("_by_lua_block") {
            let Some(lua) = directive.lua() else {
                self.error_with_help(
                    file,
                    directive.span,
                    codes::ARGUMENTS,
                    format!("'{name}' needs its Lua code in braces"),
                    format!("write it as `{name} {{ ... }}`"),
                );
                return None;
            };
            let text = self.sources.get(file).unwrap_or_default();
            let line = self
                .indexes
                .get(file)
                .map_or(1, |index| index.position(text, lua.span.start).0);
            return Some(LuaCode::Inline {
                code: lua.code.clone(),
                file: Some(file.to_owned()),
                line: u32::try_from(line).unwrap_or(u32::MAX),
            });
        }
        let arg = &directive.args[0];
        let path = self.value(file, arg)?;
        if !path.starts_with(LUA_DIRECTORY) || !path.ends_with(".lua") {
            self.error_with_help(
                file,
                arg.span,
                codes::TYPE,
                format!("{path:?} is not a Lua file of the configuration"),
                "scripts are .lua files under lua/, such as lua/auth.lua",
            );
            return None;
        }
        if self.sources.get(&path).is_none() {
            self.error_with_help(
                file,
                arg.span,
                codes::REFERENCE,
                format!("{path} is not a file of this configuration"),
                format!("add {path}, or name a file that exists"),
            );
            return None;
        }
        Some(LuaCode::File { path })
    }

    /// Sets the handler of `directive`'s phase in `slot`, which must not have one.
    fn lua_handler(
        &mut self,
        file: &str,
        directive: &Directive,
        slot: &mut Option<LuaCode>,
        place: &str,
    ) {
        if slot.is_some() {
            let phase = phase(&directive.name.value).unwrap_or("this");
            self.error(
                file,
                directive.name.span,
                codes::DUPLICATE,
                format!("{place} already has {phase}_by_lua"),
            );
            return;
        }
        *slot = self.lua_code(file, directive);
    }

    /// A handler or term of `http`, a server or a route.
    pub(super) fn lua_scope(
        &mut self,
        file: &str,
        directive: &Directive,
        scope: &mut LuaScope,
        place: &str,
    ) {
        let name = directive.name.value.as_str();
        let Some(arg) = directive.args.first() else {
            let slot = match phase(name) {
                Some("server_rewrite") => &mut scope.server_rewrite,
                Some("rewrite") => &mut scope.rewrite,
                Some("access") => &mut scope.access,
                Some("header_filter") => &mut scope.header_filter,
                Some("body_filter") => &mut scope.body_filter,
                Some("log") => &mut scope.log,
                _ => unreachable!("only handler blocks take no argument"),
            };
            self.lua_handler(file, directive, slot, place);
            return;
        };
        let duration = |lowerer: &mut Self| {
            let value = lowerer.value(file, arg)?;
            lowerer.duration(file, arg, &value)
        };
        match name {
            _ if name.ends_with("_by_lua_file") => {
                let slot = match phase(name) {
                    Some("server_rewrite") => &mut scope.server_rewrite,
                    Some("rewrite") => &mut scope.rewrite,
                    Some("access") => &mut scope.access,
                    Some("header_filter") => &mut scope.header_filter,
                    Some("body_filter") => &mut scope.body_filter,
                    Some("log") => &mut scope.log,
                    _ => unreachable!("the scope takes no other handler"),
                };
                self.lua_handler(file, directive, slot, place);
            }
            "lua_time_limit" => {
                if let Some(ms) = duration(self) {
                    scope.time_limit_ms = Some(ms);
                }
            }
            "lua_slow_threshold" => {
                if let Some(ms) = duration(self) {
                    scope.slow_threshold_ms = Some(ms);
                }
            }
            "lua_work_limit" => {
                if let Some(value) = self.value(file, arg) {
                    scope.work_limit = self.number(file, arg, &value, "a whole number");
                }
            }
            "lua_debug" => scope.debug = self.bool_arg(file, arg),
            "lua_allow" => scope.allow = self.lua_allow(file, directive),
            "lua_on_error" => {
                let Some(value) = self.value(file, arg) else {
                    return;
                };
                scope.on_error = match value.as_str() {
                    "fail" => Some(LuaFallback::Fail),
                    "continue" => Some(LuaFallback::Continue),
                    status => self
                        .number(file, arg, status, "fail, continue or an HTTP status")
                        .map(|status| LuaFallback::Status { status }),
                };
            }
            "lua_log_level" => {
                let Some(value) = self.value(file, arg) else {
                    return;
                };
                match LOG_LEVELS.iter().find(|(known, _)| *known == value) {
                    Some((_, level)) => scope.log_level = Some(*level),
                    None => self.error_with_help(
                        file,
                        arg.span,
                        codes::TYPE,
                        format!("{value:?} is not a log level"),
                        "use stderr, emerg, alert, crit, error, warn, notice, info or debug",
                    ),
                }
            }
            "lua_code_cache" => {
                if self.bool_arg(file, arg) == Some(false) {
                    self.error_with_help(
                        file,
                        arg.span,
                        codes::TYPE,
                        "scripts are compiled once per activation; lua_code_cache off is not available",
                        "remove it: a changed script takes effect when its configuration is activated",
                    );
                }
            }
            _ => unreachable!("the schema has no other Lua term"),
        }
    }

    fn lua_allow(&mut self, file: &str, directive: &Directive) -> Option<LuaPermissions> {
        let mut allow = LuaPermissions::default();
        if let [only] = directive.args.as_slice() {
            if only.value == "none" {
                return Some(allow);
            }
        }
        for arg in &directive.args {
            match arg.value.as_str() {
                "body" => allow.body = true,
                "upstream" => allow.upstream = true,
                "network" => allow.network = true,
                other => {
                    self.error_with_help(
                        file,
                        arg.span,
                        codes::TYPE,
                        format!("{other:?} is not something scripts can be allowed"),
                        "use body, upstream or network, or none alone",
                    );
                    return None;
                }
            }
        }
        Some(allow)
    }

    /// What only `http` writes: the switch, the shared dictionaries, the
    /// memory of each VM and what runs once in each.
    pub(super) fn lua_http(&mut self, file: &str, directive: &Directive, depth: usize) {
        self.origins
            .entry("lua".into())
            .or_insert_with(|| Self::origin(file, directive, depth));
        let name = directive.name.value.as_str();
        match name {
            "lua" => {
                if let Some(on) = self.bool_arg(file, &directive.args[0]) {
                    self.lua.disabled = !on;
                }
            }
            "lua_memory_limit" => {
                self.lua.memory_limit_bytes = self.size(file, &directive.args[0]);
            }
            "lua_shared_dict" => {
                let name_arg = &directive.args[0];
                let Some(name) = self.value(file, name_arg) else {
                    return;
                };
                let Some(capacity_bytes) = self.size(file, &directive.args[1]) else {
                    return;
                };
                if self.lua.shared_dicts.iter().any(|dict| dict.name == name) {
                    self.error(
                        file,
                        name_arg.span,
                        codes::DUPLICATE,
                        format!("shared dictionary {name} is declared twice"),
                    );
                    return;
                }
                self.lua.shared_dicts.push(LuaSharedDict {
                    name,
                    capacity_bytes,
                });
            }
            _ => {
                let mut config = std::mem::take(&mut self.lua);
                let slot = if phase(name) == Some("init") {
                    &mut config.init
                } else {
                    &mut config.init_worker
                };
                self.lua_handler(file, directive, slot, "http");
                self.lua = config;
            }
        }
    }

    /// Where problems of each Lua file are reported: the file itself.
    pub(super) fn lua_files(&mut self) {
        for (path, _) in self
            .sources
            .files()
            .filter(|(path, _)| crate::source::is_lua(path))
        {
            self.lua.files.insert(
                path.to_owned(),
                self.sources.get(path).unwrap_or_default().to_owned(),
            );
            self.origins.insert(
                path.to_owned(),
                Origin {
                    file: path.to_owned(),
                    span: panel_dsl::Span::default(),
                    outer: panel_dsl::Span::default(),
                    depth: 0,
                },
            );
        }
    }
}

/// The `lua_*` term directives `scope` sets, as they are written.
pub(crate) fn print_terms(scope: &LuaScope) -> Vec<(&'static str, Vec<String>)> {
    let mut terms = Vec::new();
    if let Some(ms) = scope.time_limit_ms {
        terms.push(("lua_time_limit", vec![values::print_duration_ms(ms)]));
    }
    if let Some(work) = scope.work_limit {
        terms.push(("lua_work_limit", vec![work.to_string()]));
    }
    if let Some(allow) = scope.allow {
        let mut granted: Vec<String> = [
            ("body", allow.body),
            ("upstream", allow.upstream),
            ("network", allow.network),
        ]
        .into_iter()
        .filter(|(_, on)| *on)
        .map(|(name, _)| name.to_owned())
        .collect();
        if granted.is_empty() {
            granted.push("none".into());
        }
        terms.push(("lua_allow", granted));
    }
    if let Some(on_error) = scope.on_error {
        let value = match on_error {
            LuaFallback::Fail => "fail".to_owned(),
            LuaFallback::Continue => "continue".to_owned(),
            LuaFallback::Status { status } => status.to_string(),
        };
        terms.push(("lua_on_error", vec![value]));
    }
    if let Some(level) = scope.log_level {
        terms.push(("lua_log_level", vec![log_level_name(level).to_owned()]));
    }
    if let Some(ms) = scope.slow_threshold_ms {
        terms.push(("lua_slow_threshold", vec![values::print_duration_ms(ms)]));
    }
    if let Some(debug) = scope.debug {
        terms.push(("lua_debug", vec![values::print_bool(debug).to_owned()]));
    }
    terms
}
