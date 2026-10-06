//! Lua directives (ADR 0039): the handlers of each phase, the terms they
//! run on, what runs once in each VM, and the shared dictionaries.

use super::{Expansion, Lowerer, Origin};
use crate::{codes, values};
use panel_config_model::{
    Action, LuaCode, LuaFallback, LuaLogLevel, LuaPermissions, LuaScope, LuaSharedDict,
    LuaVariable, LUA_DIRECTORY,
};
use panel_dsl::Directive;
use panel_errors::Diagnostic;

/// Directives that set a handler or a term of `http`, a server or a route.
pub(super) const SCOPE: &[&str] = &[
    "server_rewrite_by_lua_block",
    "server_rewrite_by_lua_file",
    "rewrite_by_lua_block",
    "rewrite_by_lua_file",
    "access_by_lua_block",
    "access_by_lua_file",
    "precontent_by_lua_block",
    "precontent_by_lua_file",
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
    "lua_socket_connect_timeout",
    "lua_socket_send_timeout",
    "lua_socket_read_timeout",
    "lua_socket_buffer_size",
    "lua_socket_pool_size",
    "lua_socket_keepalive_timeout",
    "lua_socket_log_errors",
    "lua_transform_underscores_in_response_headers",
    "lua_use_default_type",
    "lua_need_request_body",
    "lua_check_client_abort",
    "lua_ssl_trusted_certificate",
    "lua_ssl_verify_depth",
    "lua_ssl_crl",
    "lua_ssl_certificate",
    "lua_ssl_certificate_key",
    "lua_ssl_protocols",
    "lua_ssl_ciphers",
    "lua_code_cache",
];

/// The directives that set the same handler or term, in the order they are
/// explained.
pub(crate) const GROUPS: &[&[&str]] = &[
    &["server_rewrite_by_lua_block", "server_rewrite_by_lua_file"],
    &["rewrite_by_lua_block", "rewrite_by_lua_file"],
    &["access_by_lua_block", "access_by_lua_file"],
    &["precontent_by_lua_block", "precontent_by_lua_file"],
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
    &["lua_socket_connect_timeout"],
    &["lua_socket_send_timeout"],
    &["lua_socket_read_timeout"],
    &["lua_socket_buffer_size"],
    &["lua_socket_pool_size"],
    &["lua_socket_keepalive_timeout"],
    &["lua_socket_log_errors"],
    &["lua_transform_underscores_in_response_headers"],
    &["lua_use_default_type"],
    &["lua_need_request_body"],
    &["lua_check_client_abort"],
    &["lua_ssl_trusted_certificate"],
    &["lua_ssl_verify_depth"],
    &["lua_ssl_crl"],
    &["lua_ssl_certificate"],
    &["lua_ssl_certificate_key"],
    &["lua_ssl_protocols"],
    &["lua_ssl_ciphers"],
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
    ("lua_socket_connect_timeout", "60s"),
    ("lua_socket_send_timeout", "60s"),
    ("lua_socket_read_timeout", "60s"),
    ("lua_socket_buffer_size", "16k"),
    ("lua_socket_pool_size", "30"),
    ("lua_socket_keepalive_timeout", "60s"),
    ("lua_socket_log_errors", "on"),
    ("lua_transform_underscores_in_response_headers", "on"),
    ("lua_use_default_type", "on"),
    ("lua_need_request_body", "off"),
    ("lua_check_client_abort", "off"),
    ("lua_ssl_trusted_certificate", "system"),
    ("lua_ssl_protocols", "TLSv1.2 TLSv1.3"),
    ("lua_ssl_ciphers", "DEFAULT"),
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
    "lua_socket_connect_timeout",
    "lua_socket_send_timeout",
    "lua_socket_read_timeout",
    "lua_socket_buffer_size",
    "lua_socket_pool_size",
    "lua_socket_keepalive_timeout",
    "lua_socket_log_errors",
    "lua_transform_underscores_in_response_headers",
    "lua_use_default_type",
    "lua_need_request_body",
    "lua_check_client_abort",
    "lua_ssl_trusted_certificate",
    "lua_ssl_verify_depth",
    "lua_ssl_crl",
    "lua_ssl_certificate",
    "lua_ssl_certificate_key",
    "lua_ssl_protocols",
    "lua_ssl_ciphers",
];

/// OpenResty directives with nothing to tune here, and why. They are read
/// with a warning, so configurations written for OpenResty still read.
pub(crate) const INERT: &[(&str, &str)] = &[
    ("lua_load_resty_core", "lua-resty-core is always loaded, as in OpenResty since 0.10.16"),
    ("lua_malloc_trim", "the gateway's allocator returns freed memory to the system itself"),
    ("lua_sa_restart", "scripts make no system calls a signal could interrupt"),
    ("lua_thread_cache_max_entries", "VMs reuse coroutines without a cache to size"),
    ("rewrite_by_lua_no_postpone", "nothing else runs in the rewrite phase for rewrite_by_lua to wait for"),
    ("precontent_by_lua_no_postpone", "nothing else runs in the precontent phase for precontent_by_lua to wait for"),
    ("lua_http10_buffering", "the answers of scripts are always buffered and sent with a Content-Length"),
    ("lua_upstream_skip_openssl_default_verify", "cosockets verify certificates with the system's trusted roots, not OpenSSL's defaults"),
    ("balancer_keepalive", "the gateway pools connections to upstream nodes itself, and keepalive on|off turns reuse on or off"),
];

/// Why an OpenResty directive has no effect here, if it is one of those.
pub(crate) fn inert(name: &str) -> Option<&'static str> {
    INERT
        .iter()
        .find(|(inert, _)| *inert == name)
        .map(|(_, why)| *why)
}

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
    "access_by_lua_no_postpone",
    "lua_max_pending_timers",
    "lua_max_running_timers",
    "lua_regex_cache_max_entries",
    "lua_regex_match_limit",
    "lua_worker_thread_vm_pool_size",
    "lua_capture_error_log",
    "init_by_lua_block",
    "init_by_lua_file",
    "init_worker_by_lua_block",
    "init_worker_by_lua_file",
    "exit_worker_by_lua_block",
    "exit_worker_by_lua_file",
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
        "precontent",
        "content",
        "balancer",
        "header_filter",
        "body_filter",
        "log",
        "init",
        "init_worker",
        "exit_worker",
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
        self.lua_code_at(file, directive, 0)
    }

    /// The code of a `*_by_lua_block` directive, or of the file the
    /// argument at `path` of a `*_by_lua_file` directive names.
    fn lua_code_at(&mut self, file: &str, directive: &Directive, path: usize) -> Option<LuaCode> {
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
        let arg = &directive.args[path];
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

    /// `set_by_lua_block $name [argument ...] { ... }` and
    /// `set_by_lua_file $name lua/<file>.lua [argument ...]`: a variable of
    /// the request a script sets as the request reaches the block.
    pub(super) fn set_by_lua(&mut self, file: &str, directive: &Directive) {
        let Some(name) = self.variable_name(file, &directive.args[0]) else {
            return;
        };
        let block = directive.name.value.ends_with("_block");
        let Some(code) = self.lua_code_at(file, directive, 1) else {
            return;
        };
        let mut args = Vec::new();
        for arg in &directive.args[if block { 1 } else { 2 }..] {
            let Some(value) = self.expand(file, arg, &arg.value.clone(), Expansion::Template)
            else {
                return;
            };
            if let Err(error) = panel_ir::template::parse_template(&value) {
                self.error(file, arg.span, codes::TYPE, error);
                return;
            }
            args.push(value);
        }
        let scope = self.scopes.last_mut().expect("a scope");
        scope.scripted.insert(name.clone());
        scope.variables.push(LuaVariable::script(name, code, args));
    }

    /// An OpenResty directive with nothing to tune here.
    pub(super) fn lua_inert(&mut self, file: &str, directive: &Directive) {
        let name = directive.name.value.as_str();
        let Some(why) = inert(name) else {
            return;
        };
        let diagnostic =
            Diagnostic::warning(codes::NO_EFFECT, format!("'{name}' has no effect: {why}"))
                .with_help("remove it");
        self.report(diagnostic, file, directive.name.span);
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
                Some("precontent") => &mut scope.precontent,
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
                    Some("precontent") => &mut scope.precontent,
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
            "lua_socket_connect_timeout" => scope.socket_connect_timeout_ms = duration(self),
            "lua_socket_send_timeout" => scope.socket_send_timeout_ms = duration(self),
            "lua_socket_read_timeout" => scope.socket_read_timeout_ms = duration(self),
            "lua_socket_keepalive_timeout" => scope.socket_keepalive_timeout_ms = duration(self),
            "lua_socket_buffer_size" => scope.socket_buffer_bytes = self.size(file, arg),
            "lua_socket_pool_size" => {
                if let Some(value) = self.value(file, arg) {
                    scope.socket_pool_size = self.number(file, arg, &value, "a whole number");
                }
            }
            "lua_socket_log_errors" => scope.socket_log_errors = self.bool_arg(file, arg),
            "lua_transform_underscores_in_response_headers" => {
                scope.transform_underscores = self.bool_arg(file, arg);
            }
            "lua_use_default_type" => scope.use_default_type = self.bool_arg(file, arg),
            "lua_need_request_body" => scope.need_request_body = self.bool_arg(file, arg),
            "lua_check_client_abort" => scope.check_client_abort = self.bool_arg(file, arg),
            "lua_ssl_trusted_certificate" => scope.ssl_trusted_certificate = self.value(file, arg),
            "lua_ssl_crl" => scope.ssl_crl = self.value(file, arg),
            "lua_ssl_certificate" => scope.ssl_certificate = self.value(file, arg),
            "lua_ssl_certificate_key" => scope.ssl_certificate_key = self.value(file, arg),
            "lua_ssl_verify_depth" => {
                if let Some(value) = self.value(file, arg) {
                    scope.ssl_verify_depth = self.number(file, arg, &value, "a whole number");
                }
            }
            "lua_ssl_protocols" => scope.ssl_protocols = self.lua_ssl_protocols(file, directive),
            "lua_ssl_ciphers" => {
                if let Some(value) = self.value(file, arg) {
                    match panel_ir::tls::openssl_suites(&value) {
                        Ok(suites) => scope.ssl_ciphers = Some(suites),
                        Err(error) => self.error(file, arg.span, codes::TYPE, error),
                    }
                }
            }
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

    /// The versions `lua_ssl_protocols` offers; those rustls does not speak
    /// are warned about.
    fn lua_ssl_protocols(&mut self, file: &str, directive: &Directive) -> Option<Vec<String>> {
        let mut offered: Vec<String> = Vec::new();
        for arg in &directive.args {
            match arg.value.as_str() {
                known @ ("TLSv1.2" | "TLSv1.3") => {
                    if !offered.iter().any(|protocol| protocol == known) {
                        offered.push(known.to_owned());
                    }
                }
                old @ ("SSLv2" | "SSLv3" | "TLSv1" | "TLSv1.1") => {
                    let diagnostic = Diagnostic::warning(
                        codes::NO_EFFECT,
                        format!("{old} is not offered: RFC 8996 retires TLS 1.0 and 1.1, and SSL before them"),
                    )
                    .with_help("remove it");
                    self.report(diagnostic, file, arg.span);
                }
                other => {
                    self.error_with_help(
                        file,
                        arg.span,
                        codes::TYPE,
                        format!("{other:?} is not a protocol version"),
                        "use TLSv1.2 or TLSv1.3",
                    );
                    return None;
                }
            }
        }
        if offered.is_empty() {
            self.error_with_help(
                file,
                directive.span,
                codes::TYPE,
                "lua_ssl_protocols offers no version cosockets speak",
                "add TLSv1.2 or TLSv1.3",
            );
            return None;
        }
        Some(offered)
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
            "lua_capture_error_log" => {
                self.lua.capture_error_log_bytes = self.size(file, &directive.args[0]);
            }
            "access_by_lua_no_postpone" => {
                self.lua.access_no_postpone = self.bool_arg(file, &directive.args[0]);
            }
            "lua_max_pending_timers"
            | "lua_max_running_timers"
            | "lua_regex_cache_max_entries"
            | "lua_regex_match_limit"
            | "lua_worker_thread_vm_pool_size" => {
                let arg = &directive.args[0];
                let Some(value) = self.value(file, arg) else {
                    return;
                };
                let number = self.number(file, arg, &value, "a whole number");
                let field = match name {
                    "lua_max_pending_timers" => &mut self.lua.max_pending_timers,
                    "lua_max_running_timers" => &mut self.lua.max_running_timers,
                    "lua_regex_cache_max_entries" => &mut self.lua.regex_cache_max_entries,
                    "lua_worker_thread_vm_pool_size" => &mut self.lua.worker_thread_vm_pool_size,
                    _ => &mut self.lua.regex_match_limit,
                };
                *field = number;
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
                let slot = match phase(name) {
                    Some("init") => &mut config.init,
                    Some("exit_worker") => &mut config.exit_worker,
                    _ => &mut config.init_worker,
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
    for (name, ms) in [
        (
            "lua_socket_connect_timeout",
            scope.socket_connect_timeout_ms,
        ),
        ("lua_socket_send_timeout", scope.socket_send_timeout_ms),
        ("lua_socket_read_timeout", scope.socket_read_timeout_ms),
    ] {
        if let Some(ms) = ms {
            terms.push((name, vec![values::print_duration_ms(ms)]));
        }
    }
    if let Some(bytes) = scope.socket_buffer_bytes {
        terms.push(("lua_socket_buffer_size", vec![values::print_size(bytes)]));
    }
    if let Some(size) = scope.socket_pool_size {
        terms.push(("lua_socket_pool_size", vec![size.to_string()]));
    }
    if let Some(ms) = scope.socket_keepalive_timeout_ms {
        terms.push((
            "lua_socket_keepalive_timeout",
            vec![values::print_duration_ms(ms)],
        ));
    }
    for (name, on) in [
        ("lua_socket_log_errors", scope.socket_log_errors),
        (
            "lua_transform_underscores_in_response_headers",
            scope.transform_underscores,
        ),
        ("lua_use_default_type", scope.use_default_type),
        ("lua_need_request_body", scope.need_request_body),
        ("lua_check_client_abort", scope.check_client_abort),
    ] {
        if let Some(on) = on {
            terms.push((name, vec![values::print_bool(on).to_owned()]));
        }
    }
    for (name, secret) in [
        (
            "lua_ssl_trusted_certificate",
            &scope.ssl_trusted_certificate,
        ),
        ("lua_ssl_crl", &scope.ssl_crl),
        ("lua_ssl_certificate", &scope.ssl_certificate),
        ("lua_ssl_certificate_key", &scope.ssl_certificate_key),
    ] {
        if let Some(secret) = secret {
            terms.push((name, vec![crate::variables::escape(secret).into_owned()]));
        }
    }
    if let Some(depth) = scope.ssl_verify_depth {
        terms.push(("lua_ssl_verify_depth", vec![depth.to_string()]));
    }
    if let Some(protocols) = &scope.ssl_protocols {
        terms.push(("lua_ssl_protocols", protocols.clone()));
    }
    if let Some(suites) = &scope.ssl_ciphers {
        let names: Vec<&str> = panel_ir::tls::OPENSSL_SUITES
            .iter()
            .filter(|(_, iana)| suites.iter().any(|suite| suite == iana))
            .map(|(openssl, _)| *openssl)
            .collect();
        let list = if names.is_empty() {
            "DEFAULT".to_owned()
        } else {
            names.join(":")
        };
        terms.push(("lua_ssl_ciphers", vec![list]));
    }
    terms
}
