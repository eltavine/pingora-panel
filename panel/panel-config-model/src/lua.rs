//! Lua scripts (ADR 0039): the code each level of the configuration runs and
//! the terms it runs on. A handler or term a level sets replaces the one of
//! the level around it, as NGINX inherits `*_by_lua*` directives: `http`,
//! then the site, then the route.

use crate::model::{Action, ConfigModel};
use panel_domain::ContentHash;
pub use panel_ir::{LuaFallback, LuaLogLevel, LuaPermissions, LuaSharedDict};
use panel_ir::{LuaHandler, LuaHandlers, LuaScript, LuaSockets, LuaTls};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

/// The directory of the configuration that holds its Lua files.
pub const LUA_DIRECTORY: &str = "lua/";

/// What `lua_ssl_trusted_certificate` names for the system's trusted roots.
pub const SYSTEM_ROOTS: &str = "system";

fn is_false(value: &bool) -> bool {
    !*value
}

const fn first_line() -> u32 {
    1
}

fn is_first_line(line: &u32) -> bool {
    *line == 1
}

/// A handler's code.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LuaCode {
    /// Code written where the handler is declared.
    Inline {
        code: String,
        /// The configuration file it is written in, for messages.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file: Option<String>,
        /// The line of `file` the code starts on.
        #[serde(default = "first_line", skip_serializing_if = "is_first_line")]
        line: u32,
    },
    /// A file of the configuration under `lua/`.
    File { path: String },
}

impl LuaCode {
    pub fn inline(code: impl Into<String>) -> Self {
        Self::Inline {
            code: code.into(),
            file: None,
            line: 1,
        }
    }

    pub fn file(path: impl Into<String>) -> Self {
        Self::File { path: path.into() }
    }
}

/// A variable of the request a level sets as the request reaches it, as
/// `set` and `set_by_lua*` do in nginx: to a value, or to what a script
/// returns. Templates name it as `${lua:NAME}` and scripts as
/// `ngx.var.NAME`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct LuaVariable {
    pub name: String,
    /// What `set` gives it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    /// The `set_by_lua` script whose result it is set to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<LuaCode>,
    /// Templates filled in for the request: the script's `ngx.arg`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

impl LuaVariable {
    pub fn value(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: Some(value.into()),
            code: None,
            args: Vec::new(),
        }
    }

    pub fn script(name: impl Into<String>, code: LuaCode, args: Vec<String>) -> Self {
        Self {
            name: name.into(),
            value: None,
            code: Some(code),
            args,
        }
    }
}

/// What one level runs and the terms its handlers run on; what it leaves
/// unset comes from the level around it. Its variables are its own: those
/// of `http` and a site are set before the site's `server_rewrite`, a
/// route's before its `rewrite`.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct LuaScope {
    /// Before the route is chosen; `http` and sites only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_rewrite: Option<LuaCode>,
    /// After the route is chosen, before security policies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rewrite: Option<LuaCode>,
    /// After security policies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<LuaCode>,
    /// After access, before the action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precontent: Option<LuaCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_filter: Option<LuaCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_filter: Option<LuaCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<LuaCode>,
    /// As a TLS handshake's hello arrives, for the site its server name
    /// selects; `http` and sites only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssl_client_hello: Option<LuaCode>,
    /// As a TLS handshake chooses the certificate it presents; `http` and
    /// sites only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssl_cert: Option<LuaCode>,
    /// Wall-clock milliseconds a run may take, waits included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_limit_ms: Option<u64>,
    /// Function calls and loop iterations a run may make.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow: Option<LuaPermissions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_error: Option<LuaFallback>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_level: Option<LuaLogLevel>,
    /// Runs longer than this many milliseconds are logged and counted as
    /// slow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slow_threshold_ms: Option<u64>,
    /// Logs the start, end, duration and outcome of every run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug: Option<bool>,
    /// `lua_socket_connect_timeout`, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket_connect_timeout_ms: Option<u64>,
    /// `lua_socket_send_timeout`, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket_send_timeout_ms: Option<u64>,
    /// `lua_socket_read_timeout`, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket_read_timeout_ms: Option<u64>,
    /// `lua_socket_buffer_size`, in bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket_buffer_bytes: Option<u64>,
    /// `lua_socket_pool_size`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket_pool_size: Option<u64>,
    /// `lua_socket_keepalive_timeout`, in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket_keepalive_timeout_ms: Option<u64>,
    /// `lua_socket_log_errors`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub socket_log_errors: Option<bool>,
    /// `lua_transform_underscores_in_response_headers`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform_underscores: Option<bool>,
    /// `lua_use_default_type`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_default_type: Option<bool>,
    /// `lua_need_request_body`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub need_request_body: Option<bool>,
    /// `lua_check_client_abort`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_client_abort: Option<bool>,
    /// `lua_ssl_trusted_certificate`: the secret of the authorities
    /// cosockets trust, or [`SYSTEM_ROOTS`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssl_trusted_certificate: Option<String>,
    /// `lua_ssl_crl`: the secret of their revocation lists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssl_crl: Option<String>,
    /// `lua_ssl_certificate`: the secret of the chain cosockets present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssl_certificate: Option<String>,
    /// `lua_ssl_certificate_key`: the secret of its private key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssl_certificate_key: Option<String>,
    /// `lua_ssl_verify_depth`: the most intermediate certificates a server
    /// may send.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssl_verify_depth: Option<u64>,
    /// `lua_ssl_protocols`: `TLSv1.2` and `TLSv1.3`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssl_protocols: Option<Vec<String>>,
    /// `lua_ssl_ciphers`: IANA names of the TLS 1.2 suites offered; all
    /// when empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssl_ciphers: Option<Vec<String>>,
    /// Set in order as the request reaches the level.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variables: Vec<LuaVariable>,
}

impl LuaScope {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// This level's handlers and terms over those of `outer`.
    pub fn over(&self, outer: &Self) -> Self {
        fn pick<T: Clone>(inner: &Option<T>, outer: &Option<T>) -> Option<T> {
            inner.clone().or_else(|| outer.clone())
        }
        Self {
            server_rewrite: pick(&self.server_rewrite, &outer.server_rewrite),
            rewrite: pick(&self.rewrite, &outer.rewrite),
            access: pick(&self.access, &outer.access),
            precontent: pick(&self.precontent, &outer.precontent),
            header_filter: pick(&self.header_filter, &outer.header_filter),
            body_filter: pick(&self.body_filter, &outer.body_filter),
            log: pick(&self.log, &outer.log),
            ssl_client_hello: pick(&self.ssl_client_hello, &outer.ssl_client_hello),
            ssl_cert: pick(&self.ssl_cert, &outer.ssl_cert),
            time_limit_ms: pick(&self.time_limit_ms, &outer.time_limit_ms),
            work_limit: pick(&self.work_limit, &outer.work_limit),
            allow: pick(&self.allow, &outer.allow),
            on_error: pick(&self.on_error, &outer.on_error),
            log_level: pick(&self.log_level, &outer.log_level),
            slow_threshold_ms: pick(&self.slow_threshold_ms, &outer.slow_threshold_ms),
            debug: pick(&self.debug, &outer.debug),
            socket_connect_timeout_ms: pick(
                &self.socket_connect_timeout_ms,
                &outer.socket_connect_timeout_ms,
            ),
            socket_send_timeout_ms: pick(
                &self.socket_send_timeout_ms,
                &outer.socket_send_timeout_ms,
            ),
            socket_read_timeout_ms: pick(
                &self.socket_read_timeout_ms,
                &outer.socket_read_timeout_ms,
            ),
            socket_buffer_bytes: pick(&self.socket_buffer_bytes, &outer.socket_buffer_bytes),
            socket_pool_size: pick(&self.socket_pool_size, &outer.socket_pool_size),
            socket_keepalive_timeout_ms: pick(
                &self.socket_keepalive_timeout_ms,
                &outer.socket_keepalive_timeout_ms,
            ),
            socket_log_errors: pick(&self.socket_log_errors, &outer.socket_log_errors),
            transform_underscores: pick(&self.transform_underscores, &outer.transform_underscores),
            use_default_type: pick(&self.use_default_type, &outer.use_default_type),
            need_request_body: pick(&self.need_request_body, &outer.need_request_body),
            check_client_abort: pick(&self.check_client_abort, &outer.check_client_abort),
            ssl_trusted_certificate: pick(
                &self.ssl_trusted_certificate,
                &outer.ssl_trusted_certificate,
            ),
            ssl_crl: pick(&self.ssl_crl, &outer.ssl_crl),
            ssl_certificate: pick(&self.ssl_certificate, &outer.ssl_certificate),
            ssl_certificate_key: pick(&self.ssl_certificate_key, &outer.ssl_certificate_key),
            ssl_verify_depth: pick(&self.ssl_verify_depth, &outer.ssl_verify_depth),
            ssl_protocols: pick(&self.ssl_protocols, &outer.ssl_protocols),
            ssl_ciphers: pick(&self.ssl_ciphers, &outer.ssl_ciphers),
            variables: self.variables.clone(),
        }
    }

    /// The handlers it declares, with the names of their phases; those of
    /// its variables run in `set`.
    pub fn handlers(&self) -> impl Iterator<Item = (&'static str, &LuaCode)> {
        [
            ("server_rewrite", &self.server_rewrite),
            ("rewrite", &self.rewrite),
            ("access", &self.access),
            ("precontent", &self.precontent),
            ("header_filter", &self.header_filter),
            ("body_filter", &self.body_filter),
            ("log", &self.log),
            ("ssl_client_hello", &self.ssl_client_hello),
            ("ssl_cert", &self.ssl_cert),
        ]
        .into_iter()
        .filter_map(|(phase, code)| code.as_ref().map(|code| (phase, code)))
        .chain(
            self.variables
                .iter()
                .filter_map(|variable| variable.code.as_ref().map(|code| ("set", code))),
        )
    }

    fn handlers_mut(&mut self) -> impl Iterator<Item = &mut LuaCode> {
        [
            &mut self.server_rewrite,
            &mut self.rewrite,
            &mut self.access,
            &mut self.precontent,
            &mut self.header_filter,
            &mut self.body_filter,
            &mut self.log,
            &mut self.ssl_client_hello,
            &mut self.ssl_cert,
        ]
        .into_iter()
        .flatten()
        .chain(
            self.variables
                .iter_mut()
                .filter_map(|variable| variable.code.as_mut()),
        )
    }

    /// Whether it sets any term, as opposed to only handlers.
    pub fn has_terms(&self) -> bool {
        self.time_limit_ms.is_some()
            || self.work_limit.is_some()
            || self.allow.is_some()
            || self.on_error.is_some()
            || self.log_level.is_some()
            || self.slow_threshold_ms.is_some()
            || self.debug.is_some()
            || self.socket_connect_timeout_ms.is_some()
            || self.socket_send_timeout_ms.is_some()
            || self.socket_read_timeout_ms.is_some()
            || self.socket_buffer_bytes.is_some()
            || self.socket_pool_size.is_some()
            || self.socket_keepalive_timeout_ms.is_some()
            || self.socket_log_errors.is_some()
            || self.transform_underscores.is_some()
            || self.use_default_type.is_some()
            || self.need_request_body.is_some()
            || self.check_client_abort.is_some()
            || self.ssl_trusted_certificate.is_some()
            || self.ssl_crl.is_some()
            || self.ssl_certificate.is_some()
            || self.ssl_certificate_key.is_some()
            || self.ssl_verify_depth.is_some()
            || self.ssl_protocols.is_some()
            || self.ssl_ciphers.is_some()
    }
}

/// Lua across the configuration: what runs once in each VM, the shared
/// dictionaries, what every site runs and the files scripts load.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct LuaConfig {
    /// `lua off`: the scripts are kept and checked, but none runs.
    #[serde(default, skip_serializing_if = "is_false")]
    pub disabled: bool,
    /// Runs once in each VM when the configuration is activated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub init: Option<LuaCode>,
    /// Runs once in each VM after `init`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub init_worker: Option<LuaCode>,
    /// Runs once in each VM when it stops.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_worker: Option<LuaCode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared_dicts: Vec<LuaSharedDict>,
    /// Bytes each VM may allocate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_limit_bytes: Option<u64>,
    /// Timers each VM may hold waiting (`lua_max_pending_timers`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_pending_timers: Option<u64>,
    /// Timers each VM may run at once (`lua_max_running_timers`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_running_timers: Option<u64>,
    /// Compiled regular expressions each VM keeps
    /// (`lua_regex_cache_max_entries`); zero keeps none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex_cache_max_entries: Option<u64>,
    /// PCRE2's match limit for `ngx.re` (`lua_regex_match_limit`); zero
    /// keeps PCRE2's own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex_match_limit: Option<u64>,
    /// Bytes of what each VM's scripts log that `ngx.errlog` reads back
    /// (`lua_capture_error_log`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_error_log_bytes: Option<u64>,
    /// The VMs `ngx.run_worker_thread` may run on at once
    /// (`lua_worker_thread_vm_pool_size`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_thread_vm_pool_size: Option<u64>,
    /// `access_by_lua_no_postpone`: access handlers run before the
    /// security policies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_no_postpone: Option<bool>,
    /// What every site runs, and the terms of every handler; balancers and
    /// `init` run on these terms.
    #[serde(default, skip_serializing_if = "LuaScope::is_empty")]
    pub http: LuaScope,
    /// The configuration's files under `lua/` by path: the files handlers
    /// name and the modules `require` loads.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, String>,
}

impl LuaConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Every handler of `http`, the upstreams and the live sites: the resource
/// it is declared on (`lua` for `http`), its phase and its code.
pub fn lua_handlers(model: &ConfigModel) -> Vec<(String, &'static str, &LuaCode)> {
    let lua = &model.lua;
    let mut found: Vec<(String, &'static str, &LuaCode)> = Vec::new();
    for (phase, code) in [
        ("init", &lua.init),
        ("init_worker", &lua.init_worker),
        ("exit_worker", &lua.exit_worker),
    ] {
        if let Some(code) = code {
            found.push(("lua".into(), phase, code));
        }
    }
    found.extend(
        lua.http
            .handlers()
            .map(|(phase, code)| ("lua".to_owned(), phase, code)),
    );
    for upstream in &model.upstreams {
        if let Some(code) = &upstream.balancer {
            found.push((format!("upstreams/{}", upstream.id), "balancer", code));
        }
    }
    for site in model.sites.iter().filter(|site| !site.is_deleted()) {
        let resource = format!("sites/{}", site.id);
        found.extend(
            site.lua
                .handlers()
                .map(|(phase, code)| (resource.clone(), phase, code)),
        );
        if let Action::Lua { code } = &site.action {
            found.push((resource.clone(), "content", code));
        }
        for route in &site.routes {
            let resource = format!("{resource}/routes/{}", route.id);
            found.extend(
                route
                    .lua
                    .handlers()
                    .map(|(phase, code)| (resource.clone(), phase, code)),
            );
            if let Action::Lua { code } = &route.action {
                found.push((resource, "content", code));
            }
        }
    }
    found
}

/// Every handler's code in `model`, deleted sites included.
pub fn lua_codes_mut(model: &mut ConfigModel) -> Vec<&mut LuaCode> {
    let lua = &mut model.lua;
    let mut found: Vec<&mut LuaCode> = lua
        .init
        .iter_mut()
        .chain(lua.init_worker.iter_mut())
        .chain(lua.exit_worker.iter_mut())
        .collect();
    found.extend(lua.http.handlers_mut());
    found.extend(
        model
            .upstreams
            .iter_mut()
            .filter_map(|upstream| upstream.balancer.as_mut()),
    );
    for site in &mut model.sites {
        found.extend(site.lua.handlers_mut());
        if let Action::Lua { code } = &mut site.action {
            found.push(code);
        }
        for route in &mut site.routes {
            found.extend(route.lua.handlers_mut());
            if let Action::Lua { code } = &mut route.action {
                found.push(code);
            }
        }
    }
    found
}

/// What differs in Lua between two models, where inline code is written
/// aside.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LuaChanges {
    /// What every site shares: `http`'s Lua, the files and the upstreams'
    /// balancers.
    pub shared: bool,
    /// The sites whose own handlers, terms or scripted actions differ,
    /// sites added or removed with any included.
    pub sites: BTreeSet<Uuid>,
}

impl LuaChanges {
    pub fn is_empty(&self) -> bool {
        !self.shared && self.sites.is_empty()
    }
}

/// The Lua of `model` with inline code taken from where it is written.
fn placeless(model: &ConfigModel) -> ConfigModel {
    let mut model = model.clone();
    for code in lua_codes_mut(&mut model) {
        if let LuaCode::Inline { file, line, .. } = code {
            *file = None;
            *line = 1;
        }
    }
    model
}

/// The Lua `before` and `after` differ in.
pub fn lua_changes(before: &ConfigModel, after: &ConfigModel) -> LuaChanges {
    let (before, after) = (placeless(before), placeless(after));
    let balancers = |model: &ConfigModel| -> BTreeMap<Uuid, Option<LuaCode>> {
        model
            .upstreams
            .iter()
            .filter(|upstream| upstream.balancer.is_some())
            .map(|upstream| (upstream.id, upstream.balancer.clone()))
            .collect()
    };
    type SiteLua = (
        LuaScope,
        Option<LuaCode>,
        Vec<(Uuid, LuaScope, Option<LuaCode>)>,
    );
    let scripted = |action: &Action| match action {
        Action::Lua { code } => Some(code.clone()),
        _ => None,
    };
    let sites = |model: &ConfigModel| -> BTreeMap<Uuid, SiteLua> {
        model
            .sites
            .iter()
            .map(|site| {
                let routes: Vec<(Uuid, LuaScope, Option<LuaCode>)> = site
                    .routes
                    .iter()
                    .filter(|route| !route.lua.is_empty() || scripted(&route.action).is_some())
                    .map(|route| (route.id, route.lua.clone(), scripted(&route.action)))
                    .collect();
                (site.id, (site.lua.clone(), scripted(&site.action), routes))
            })
            .filter(|(_, (scope, action, routes))| {
                !scope.is_empty() || action.is_some() || !routes.is_empty()
            })
            .collect()
    };
    let (old_sites, new_sites) = (sites(&before), sites(&after));
    LuaChanges {
        shared: before.lua != after.lua || balancers(&before) != balancers(&after),
        sites: old_sites
            .keys()
            .chain(new_sites.keys())
            .filter(|id| old_sites.get(id) != new_sites.get(id))
            .copied()
            .collect(),
    }
}

/// The name `require` loads a file under `lua/` by: `lua/a/b.lua` is `a.b`
/// and `lua/a/init.lua` is `a`, as `?.lua;?/init.lua` resolve in OpenResty.
pub fn module_name(path: &str) -> Option<String> {
    let relative = path.strip_prefix(LUA_DIRECTORY)?.strip_suffix(".lua")?;
    let relative = relative.strip_suffix("/init").unwrap_or(relative);
    let valid = !relative.is_empty()
        && relative.split('/').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        });
    valid.then(|| relative.replace('/', "."))
}

/// The identifier a snapshot gives the script of `code`: the path of a
/// file, `file:line` of code written in the configuration.
pub fn script_id(code: &LuaCode) -> String {
    match code {
        LuaCode::Inline {
            file: Some(file),
            line,
            ..
        } => format!("{file}:{line}"),
        LuaCode::Inline {
            code, file: None, ..
        } => format!("inline:{}", &content_hash(code)[..16]),
        LuaCode::File { path } => path.clone(),
    }
}

/// Scripts of a snapshot being compiled, by id.
#[derive(Default)]
pub(crate) struct Scripts {
    scripts: BTreeMap<String, LuaScript>,
}

impl Scripts {
    /// The id of the script `code` is, added on first use. A file the model
    /// lacks is reported by validation, so it compiles as empty here.
    pub(crate) fn add(&mut self, code: &LuaCode, files: &BTreeMap<String, String>) -> String {
        let id = script_id(code);
        let (file, line, source) = match code {
            LuaCode::Inline { code, file, line } => (
                file.clone().unwrap_or_else(|| "inline".into()),
                *line,
                code.clone(),
            ),
            LuaCode::File { path } => (
                path.clone(),
                1,
                files.get(path).cloned().unwrap_or_default(),
            ),
        };
        self.scripts.entry(id.clone()).or_insert_with(|| LuaScript {
            id: id.clone(),
            file,
            line,
            sha256: content_hash(&source),
            module: None,
            source,
        });
        id
    }

    /// Adds every file as the module `require` loads it by.
    pub(crate) fn modules(&mut self, files: &BTreeMap<String, String>) {
        for (path, source) in files {
            let Some(name) = module_name(path) else {
                continue;
            };
            let script = self
                .scripts
                .entry(path.clone())
                .or_insert_with(|| LuaScript {
                    id: path.clone(),
                    file: path.clone(),
                    line: 1,
                    sha256: content_hash(source),
                    module: None,
                    source: source.clone(),
                });
            script.module = Some(name);
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.scripts.is_empty()
    }

    pub(crate) fn into_scripts(self) -> Vec<LuaScript> {
        self.scripts.into_values().collect()
    }
}

fn content_hash(source: &str) -> String {
    ContentHash::from_bytes(source.as_bytes())
        .as_str()
        .to_owned()
}

/// The handler running `id` on the terms of `scope`.
pub(crate) fn handler(id: String, scope: &LuaScope) -> LuaHandler {
    let mut handler = LuaHandler::new(id);
    handler.time_limit_ms = scope.time_limit_ms.unwrap_or(0);
    handler.work_limit = scope.work_limit.unwrap_or(0);
    handler.allow = scope.allow.unwrap_or_default();
    handler.on_error = scope.on_error.unwrap_or_default();
    handler.log_level = scope.log_level.unwrap_or_default();
    handler.slow_threshold_ms = scope.slow_threshold_ms.unwrap_or(0);
    handler.debug = scope.debug.unwrap_or(false);
    handler.sockets = LuaSockets {
        connect_timeout_ms: scope.socket_connect_timeout_ms.unwrap_or(0),
        send_timeout_ms: scope.socket_send_timeout_ms.unwrap_or(0),
        read_timeout_ms: scope.socket_read_timeout_ms.unwrap_or(0),
        buffer_bytes: scope.socket_buffer_bytes.unwrap_or(0),
        pool_size: scope.socket_pool_size.unwrap_or(0),
        keepalive_timeout_ms: scope.socket_keepalive_timeout_ms.unwrap_or(0),
        quiet: scope.socket_log_errors == Some(false),
        tls: Box::new(LuaTls {
            trusted_certificate_secret_id: scope
                .ssl_trusted_certificate
                .clone()
                .filter(|secret| secret != SYSTEM_ROOTS),
            crl_secret_id: scope.ssl_crl.clone(),
            certificate_secret_id: scope.ssl_certificate.clone(),
            certificate_key_secret_id: scope.ssl_certificate_key.clone(),
            verify_depth: scope
                .ssl_verify_depth
                .map(|depth| u32::try_from(depth).unwrap_or(u32::MAX)),
            protocols: scope.ssl_protocols.clone().unwrap_or_default(),
            cipher_suites: scope.ssl_ciphers.clone().unwrap_or_default(),
        }),
    };
    handler.keep_underscores = scope.transform_underscores == Some(false);
    handler.no_default_type = scope.use_default_type == Some(false);
    handler.read_body_first = scope.need_request_body == Some(true);
    handler.check_client_abort = scope.check_client_abort == Some(true);
    handler
}

/// `variables` in order, their scripts running on the terms of `scope`.
pub(crate) fn variables<'v>(
    variables: impl IntoIterator<Item = &'v LuaVariable>,
    scope: &LuaScope,
    scripts: &mut Scripts,
    files: &BTreeMap<String, String>,
) -> Vec<panel_ir::LuaVariable> {
    variables
        .into_iter()
        .map(|variable| panel_ir::LuaVariable {
            name: variable.name.clone(),
            value: variable.value.clone().unwrap_or_default(),
            handler: variable
                .code
                .as_ref()
                .map(|code| handler(scripts.add(code, files), scope)),
            args: variable.args.clone(),
        })
        .collect()
}

/// The handlers `scope` runs; a route's never include `server_rewrite` or
/// the TLS handshake's.
pub(crate) fn handlers(
    scope: &LuaScope,
    scripts: &mut Scripts,
    files: &BTreeMap<String, String>,
    route: bool,
) -> LuaHandlers {
    let mut compile = |code: &Option<LuaCode>| {
        code.as_ref()
            .map(|code| handler(scripts.add(code, files), scope))
    };
    LuaHandlers {
        server_rewrite: if route {
            None
        } else {
            compile(&scope.server_rewrite)
        },
        rewrite: compile(&scope.rewrite),
        access: compile(&scope.access),
        precontent: compile(&scope.precontent),
        header_filter: compile(&scope.header_filter),
        body_filter: compile(&scope.body_filter),
        log: compile(&scope.log),
        ssl_client_hello: if route {
            None
        } else {
            compile(&scope.ssl_client_hello)
        },
        ssl_cert: if route {
            None
        } else {
            compile(&scope.ssl_cert)
        },
        variables: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_under_lua_are_modules_by_their_path() {
        assert_eq!(module_name("lua/auth.lua").as_deref(), Some("auth"));
        assert_eq!(module_name("lua/a/b-c.lua").as_deref(), Some("a.b-c"));
        assert_eq!(module_name("lua/a/init.lua").as_deref(), Some("a"));
        assert_eq!(module_name("lua/init.lua").as_deref(), Some("init"));
        assert_eq!(module_name("lua/a.b.lua"), None);
        assert_eq!(module_name("other/a.lua"), None);
        assert_eq!(module_name("lua/a.txt"), None);
    }

    #[test]
    fn inner_levels_replace_what_they_set() {
        let outer = LuaScope {
            access: Some(LuaCode::inline("a")),
            log: Some(LuaCode::inline("l")),
            time_limit_ms: Some(50),
            ..LuaScope::default()
        };
        let inner = LuaScope {
            access: Some(LuaCode::file("lua/b.lua")),
            time_limit_ms: Some(5),
            debug: Some(true),
            ..LuaScope::default()
        };
        let merged = inner.over(&outer);
        assert_eq!(merged.access, Some(LuaCode::file("lua/b.lua")));
        assert_eq!(merged.log, Some(LuaCode::inline("l")));
        assert_eq!(merged.time_limit_ms, Some(5));
        assert_eq!(merged.debug, Some(true));
        assert_eq!(
            merged
                .handlers()
                .map(|(phase, _)| phase)
                .collect::<Vec<_>>(),
            ["access", "log"]
        );
    }

    #[test]
    fn changes_ignore_where_code_is_written() {
        let mut before = ConfigModel::default();
        before.lua.http.access = Some(LuaCode::Inline {
            code: "x()".into(),
            file: Some("main.conf".into()),
            line: 3,
        });
        let mut moved = before.clone();
        moved.lua.http.access = Some(LuaCode::Inline {
            code: "x()".into(),
            file: Some("main.conf".into()),
            line: 9,
        });
        assert!(lua_changes(&before, &moved).is_empty());
        let mut changed = before.clone();
        changed
            .lua
            .files
            .insert("lua/a.lua".into(), "return 1".into());
        assert!(lua_changes(&before, &changed).shared);
    }

    #[test]
    fn scripts_are_identified_by_place_or_file() {
        let files = BTreeMap::from([("lua/m.lua".to_owned(), "return 1".to_owned())]);
        let mut scripts = Scripts::default();
        let inline = LuaCode::Inline {
            code: "ngx.exit(403)".into(),
            file: Some("main.conf".into()),
            line: 7,
        };
        assert_eq!(scripts.add(&inline, &files), "main.conf:7");
        assert_eq!(scripts.add(&inline, &files), "main.conf:7");
        assert!(scripts
            .add(&LuaCode::inline("x()"), &files)
            .starts_with("inline:"));
        assert_eq!(
            scripts.add(&LuaCode::file("lua/m.lua"), &files),
            "lua/m.lua"
        );
        scripts.modules(&files);
        let scripts = scripts.into_scripts();
        assert_eq!(scripts.len(), 3);
        let module = scripts
            .iter()
            .find(|script| script.id == "lua/m.lua")
            .unwrap();
        assert_eq!(module.module.as_deref(), Some("m"));
        assert_eq!(module.source, "return 1");
        assert_eq!(module.sha256.len(), 64);
    }
}
