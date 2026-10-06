//! Lua scripts (ADR 0039): the scripts of a configuration and the handlers
//! its sites, routes and upstreams run in each phase. Inheritance is
//! resolved before the IR: a site or route carries the handlers it runs.

use serde::{Deserialize, Serialize};

/// Required by snapshots that carry scripts.
pub const LUA_SCRIPTS_CAPABILITY: &str = "lua.scripts";

fn is_false(value: &bool) -> bool {
    !*value
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

/// Lua source text and where it comes from.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LuaScript {
    /// Unique in the snapshot: the file of a module, `file:line` of an
    /// inline block.
    pub id: String,
    /// The configuration file it comes from, for messages.
    pub file: String,
    /// The line of `file` the source starts on, from 1.
    pub line: u32,
    pub source: String,
    /// Lowercase hexadecimal SHA-256 of `source`.
    pub sha256: String,
    /// What `require` loads it by: set for files under `lua/`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
}

/// What scripts may do beyond reading and changing the request and
/// response.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LuaPermissions {
    /// Read and replace request and response bodies.
    #[serde(default, skip_serializing_if = "is_false")]
    pub body: bool,
    /// Choose upstream endpoints.
    #[serde(default, skip_serializing_if = "is_false")]
    pub upstream: bool,
    /// Open sockets.
    #[serde(default, skip_serializing_if = "is_false")]
    pub network: bool,
}

impl LuaPermissions {
    pub fn is_none(&self) -> bool {
        *self == Self::default()
    }
}

/// What a handler's failure does: an error, a timeout or a limit reached.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LuaFallback {
    /// Answers 500, or 502 in the balancer, as OpenResty does.
    #[default]
    Fail,
    /// Goes on as if the handler had not run.
    Continue,
    /// Answers with this status.
    Status { status: u16 },
}

impl LuaFallback {
    pub fn is_fail(&self) -> bool {
        matches!(self, Self::Fail)
    }
}

/// `ngx.log` levels, most severe first: messages less severe than a
/// handler's level are dropped.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum LuaLogLevel {
    Stderr,
    Emerg,
    Alert,
    Crit,
    Error,
    Warn,
    #[default]
    Notice,
    Info,
    Debug,
}

impl LuaLogLevel {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// The TLS terms of `sslhandshake`, as the `lua_ssl_*` directives set them,
/// with the secrets the gateway reads them from.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LuaTls {
    /// The PEM authorities to trust; the system's trusted roots when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trusted_certificate_secret_id: Option<String>,
    /// PEM revocation lists of those authorities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crl_secret_id: Option<String>,
    /// The PEM certificate chain the client presents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate_secret_id: Option<String>,
    /// The PEM private key of that certificate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub certificate_key_secret_id: Option<String>,
    /// The most intermediate certificates a server may send; not limited
    /// when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_depth: Option<u32>,
    /// `TLSv1.2` and `TLSv1.3`; both when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub protocols: Vec<String>,
    /// IANA names of the TLS 1.2 suites offered; all when empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cipher_suites: Vec<String>,
}

impl LuaTls {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// The defaults of the cosockets a handler's scripts open, as the
/// `lua_socket_*` and `lua_ssl_*` directives set them; zero keeps the
/// gateway's.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LuaSockets {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub connect_timeout_ms: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub send_timeout_ms: u64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub read_timeout_ms: u64,
    /// Bytes a read takes from a connection at a time.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub buffer_bytes: u64,
    /// Idle connections `setkeepalive` keeps for a pool.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub pool_size: u64,
    /// How long `setkeepalive` keeps an idle connection.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub keepalive_timeout_ms: u64,
    /// Failures are not written to the error log
    /// (`lua_socket_log_errors off`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub quiet: bool,
    #[serde(default, skip_serializing_if = "is_default")]
    pub tls: Box<LuaTls>,
}

impl LuaSockets {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// A script run in a phase, and the terms it runs on.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LuaHandler {
    pub script_id: String,
    /// Wall-clock milliseconds a run may take, waits included; zero for the
    /// gateway's default.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub time_limit_ms: u64,
    /// Function calls and loop iterations a run may make; zero for the
    /// gateway's default.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub work_limit: u64,
    #[serde(default, skip_serializing_if = "LuaPermissions::is_none")]
    pub allow: LuaPermissions,
    #[serde(default, skip_serializing_if = "LuaFallback::is_fail")]
    pub on_error: LuaFallback,
    #[serde(default, skip_serializing_if = "LuaLogLevel::is_default")]
    pub log_level: LuaLogLevel,
    /// Runs longer than this are logged and counted as slow; zero for the
    /// gateway's default.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub slow_threshold_ms: u64,
    /// Logs the start, end, duration and outcome of every run.
    #[serde(default, skip_serializing_if = "is_false")]
    pub debug: bool,
    #[serde(default, skip_serializing_if = "LuaSockets::is_default")]
    pub sockets: LuaSockets,
    /// `ngx.header` keeps underscores in names
    /// (`lua_transform_underscores_in_response_headers off`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub keep_underscores: bool,
    /// An answer without a `Content-Type` gets none
    /// (`lua_use_default_type off`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub no_default_type: bool,
    /// The request body is read before the handler runs
    /// (`lua_need_request_body on`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub read_body_first: bool,
    /// The handler watches for the client closing the connection
    /// (`lua_check_client_abort on`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub check_client_abort: bool,
}

impl LuaHandler {
    pub fn new(script_id: impl Into<String>) -> Self {
        Self {
            script_id: script_id.into(),
            time_limit_ms: 0,
            work_limit: 0,
            allow: LuaPermissions::default(),
            on_error: LuaFallback::Fail,
            log_level: LuaLogLevel::Notice,
            slow_threshold_ms: 0,
            debug: false,
            sockets: LuaSockets::default(),
            keep_underscores: false,
            no_default_type: false,
            read_body_first: false,
            check_client_abort: false,
        }
    }
}

/// A variable of the request, as `set` and `set_by_lua*` give it one in
/// nginx: a value, or what a handler returns. Templates name it as
/// `${lua:NAME}` and scripts as `ngx.var.NAME`.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LuaVariable {
    pub name: String,
    /// What it is set to when no handler sets it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub value: String,
    /// The `set_by_lua` handler whose result it is set to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handler: Option<LuaHandler>,
    /// Templates filled in for the request: the handler's `ngx.arg`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

/// The handlers a site or route runs, by phase.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LuaHandlers {
    /// Before the route is chosen; a site's only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_rewrite: Option<LuaHandler>,
    /// After the route is chosen, before security policies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rewrite: Option<LuaHandler>,
    /// After security policies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<LuaHandler>,
    /// After access and the HTTP policies, before the route's action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub precontent: Option<LuaHandler>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header_filter: Option<LuaHandler>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_filter: Option<LuaHandler>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<LuaHandler>,
    /// As a TLS handshake's hello arrives, for the site its server name
    /// selects; a site's only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssl_client_hello: Option<LuaHandler>,
    /// As a TLS handshake chooses the certificate it presents; a site's
    /// only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssl_cert: Option<LuaHandler>,
    /// Set in order as the site's server rewrite phase or the route's
    /// rewrite phase begins, before its handler.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variables: Vec<LuaVariable>,
}

impl LuaHandlers {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Every handler, with the name of its phase; those of variables run in
    /// `set`.
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, &LuaHandler)> {
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
        .filter_map(|(phase, handler)| handler.as_ref().map(|handler| (phase, handler)))
        .chain(
            self.variables
                .iter()
                .filter_map(|variable| variable.handler.as_ref().map(|handler| ("set", handler))),
        )
    }
}

/// A dictionary `ngx.shared` holds for every VM of the gateway.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LuaSharedDict {
    pub name: String,
    pub capacity_bytes: u64,
}

/// The scripts of a snapshot and what runs once per VM.
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LuaProgram {
    /// `lua off`: the scripts stay but none runs.
    #[serde(default, skip_serializing_if = "is_false")]
    pub disabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scripts: Vec<LuaScript>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub init: Option<LuaHandler>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub init_worker: Option<LuaHandler>,
    /// Runs in each VM when it stops.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_worker: Option<LuaHandler>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shared_dicts: Vec<LuaSharedDict>,
    /// Bytes each VM may allocate; zero for the gateway's default.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub memory_limit_bytes: u64,
    /// Timers each VM may hold waiting, as `lua_max_pending_timers` sets
    /// them; zero for lua-nginx-module's 1024.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub max_pending_timers: u64,
    /// Timers each VM may run at once, as `lua_max_running_timers` sets
    /// them; zero for lua-nginx-module's 256.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub max_running_timers: u64,
    /// Compiled regular expressions each VM keeps, as
    /// `lua_regex_cache_max_entries` sets them: 1024 when unset, none at
    /// zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex_cache_max_entries: Option<u64>,
    /// PCRE2's match limit for `ngx.re`, as `lua_regex_match_limit` sets it;
    /// zero for PCRE2's own.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub regex_match_limit: u64,
    /// Access handlers run before the security policies, as
    /// `access_by_lua_no_postpone on` has them.
    #[serde(default, skip_serializing_if = "is_false")]
    pub access_first: bool,
    /// The VMs `ngx.run_worker_thread` may run on at once, as
    /// `lua_worker_thread_vm_pool_size` sets them; zero for 10.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub worker_thread_vm_pool_size: u64,
    /// Bytes of what each VM's scripts log that `ngx.errlog` reads back, as
    /// `lua_capture_error_log` sets them; none kept at zero.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub capture_error_log_bytes: u64,
}

impl LuaProgram {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn script(&self, id: &str) -> Option<&LuaScript> {
        self.scripts.iter().find(|script| script.id == id)
    }
}
