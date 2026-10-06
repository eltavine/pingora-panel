#![forbid(unsafe_code)]

//! The configuration operators edit — listeners, sites with their domains
//! and routes, upstreams and TLS profiles — and its compilation into the
//! engine-neutral runtime IR.
//!
//! The model is one versioned document so a later textual DSL is another
//! representation of it rather than a migration.

mod approvals;
mod compile;
mod edit;
mod http;
mod lua;
mod model;
mod query;
mod revisions;
mod security;
mod validate;
mod views;

pub use approvals::{
    ApprovalDecision, ApprovalPolicy, ApprovalPolicyInput, ApprovalRequest, ApprovalRequestList,
    ApprovalState, Assessment, PlannedChange, PolicyVersion, Risk, DEFAULT_VALID_MINUTES,
    MAX_APPROVALS, MAX_VALID_MINUTES, MAX_WINDOWS, MIN_VALID_MINUTES, REQUEST_LIFETIME,
    RESOURCE_KINDS,
};
pub use compile::compile;
pub use edit::{checked, NodeInput, RouteInput, SiteBundle, SiteInput, UpstreamInput};
pub use http::{FieldChanges, HttpPolicy};
pub use lua::{
    lua_changes, lua_codes_mut, lua_handlers, module_name, script_id, LuaChanges, LuaCode,
    LuaConfig, LuaFallback, LuaLogLevel, LuaPermissions, LuaScope, LuaSharedDict, LuaVariable,
    LUA_DIRECTORY,
};
pub use model::{
    entity_tag, Action, ConfigModel, Domain, Listener, MatchKind, Route, RouteCondition,
    RouteMatch, Site, SiteKind, TlsProfile, TlsProfileInput, Upstream, UpstreamNode, UpstreamRetry,
    ValueTest,
};
/// Whether a value is an RFC 9110 token, and why a route regular expression
/// would not compile, as the gateway judges them.
pub use panel_engine::{route_regex_error, token};
pub use query::{
    abnormal_sites, query_sites, serves_https, site_status, summarize, SitePage, SiteQuery,
    SiteSort, SiteStatus, SiteSummary, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE,
};
pub use revisions::{Revision, RevisionDetail, RevisionList, RevisionOutcome};
pub use security::{SecurityPolicy, MAX_BODY_TIMEOUT_SECONDS, MAX_RATE_PERIOD_SECONDS};
pub use validate::{validate, MAX_HEAD_TIMEOUT_SECONDS};
pub use views::{
    BatchAction, BatchRequest, DomainCheck, DomainOwner, DomainView, HttpPolicyView, ListenerView,
    RouteView, SecurityPolicyView, SiteList, SiteView, TlsProfileView, UpstreamView,
    ValidationResult,
};

/// Identifies the document format in storage, exports and imports.
pub const MODEL_VERSION: &str = "pingora.panel.config/v1";
