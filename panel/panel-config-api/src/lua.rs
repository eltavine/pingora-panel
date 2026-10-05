//! Lua tests (ADR 0039): a request described in full, run on the draft's
//! handlers or on one script in place of a phase's handler, and what each
//! handler did.

use panel_config_model::LuaPermissions;
use serde::{Deserialize, Serialize};

const fn ok() -> u16 {
    200
}

/// A header field line; a repeated name is a line of its own.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct HeaderLine {
    pub name: String,
    pub value: String,
}

/// The request a test runs on.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct LuaTestRequest {
    /// Such as `POST`; `GET` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// The host the request names, with or without its port.
    pub host: String,
    /// The path and query, such as `/api/items?tag=new`; `/` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<HeaderLine>,
    /// What `ngx.req.read_body` reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    /// The client's address, after trusted proxies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client: Option<String>,
    /// Whether it arrives over TLS.
    #[serde(default)]
    pub tls: bool,
    /// The listener it arrives on, which may serve only some sites.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listener: Option<String>,
}

/// The upstream's answer to a proxied request, which the response's filters
/// and the log handler see when no script answers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct LuaTestUpstream {
    #[serde(default = "ok")]
    pub status: u16,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<HeaderLine>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

/// One script run in place of a phase's handler, such as code being
/// written, with the draft's modules and `init`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct LuaTestScript {
    pub code: String,
    /// `server_rewrite`, `rewrite`, `access`, `content`, `balancer`,
    /// `header_filter`, `body_filter` or `log`.
    pub phase: String,
    #[serde(default)]
    pub allow: LuaPermissions,
    /// The gateway's default when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_limit_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_limit: Option<u64>,
}

/// A Lua test: the draft's handlers the request reaches, or one script.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct LuaTest {
    pub request: LuaTestRequest,
    /// The answer of the upstream a proxied request goes to; 200 with an
    /// empty body when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream: Option<LuaTestUpstream>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<LuaTestScript>,
}

/// How a handler's run ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LuaRunOutcome {
    /// The request goes on.
    Continue,
    /// The handler answered.
    Respond,
    /// The connection closes without an answer.
    Abort,
    /// An error, a timeout or a limit; the handler's fallback applied.
    Failed,
}

/// A message a script logged.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LuaTestLog {
    /// `ngx.log`'s level, such as `notice`.
    pub level: String,
    pub message: String,
}

/// Why a run failed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LuaTestFailure {
    /// `error`, `timeout`, `work`, `memory` or `refused`.
    pub kind: String,
    pub message: String,
}

/// One handler's run.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LuaTestRun {
    pub phase: String,
    /// The script, such as `lua/auth.lua` or `main.conf:12`.
    pub script: String,
    pub outcome: LuaRunOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<LuaTestFailure>,
    pub duration_us: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub logs: Vec<LuaTestLog>,
}

/// The request as the handlers left it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LuaTestRequestState {
    pub method: String,
    /// The decoded path.
    pub uri: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    pub headers: Vec<HeaderLine>,
}

/// The response the client gets.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LuaTestReply {
    pub status: u16,
    pub headers: Vec<HeaderLine>,
    /// The body as text, with invalid UTF-8 replaced.
    pub body: String,
}

/// What a Lua test did.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LuaTestResult {
    /// The draft version tested.
    pub draft_version: u64,
    /// The site and route the request reached, as the snapshot names them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_id: Option<String>,
    /// What `init_by_lua` and `init_worker_by_lua` logged.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub init_logs: Vec<LuaTestLog>,
    /// The handlers run, in order.
    pub runs: Vec<LuaTestRun>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<LuaTestRequestState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<LuaTestReply>,
    /// Whether a handler closed the connection without an answer.
    pub aborted: bool,
    /// The endpoint a balancer chose, as `host:port`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peer: Option<String>,
}
