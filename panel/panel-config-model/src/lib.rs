#![forbid(unsafe_code)]

//! The configuration operators edit — listeners, sites with their domains
//! and routes, upstreams and TLS profiles — and its compilation into the
//! engine-neutral runtime IR.
//!
//! The model is one versioned document so a later textual DSL is another
//! representation of it rather than a migration.

mod compile;
mod edit;
mod model;
mod query;
mod validate;

pub use compile::compile;
pub use edit::{checked, NodeInput, RouteInput, SiteBundle, SiteInput, UpstreamInput};
pub use model::{
    entity_tag, Action, ConfigModel, Domain, Listener, MatchKind, Route, RouteMatch, Site,
    SiteKind, Upstream, UpstreamNode,
};
pub use query::{
    abnormal_sites, query_sites, serves_https, site_status, summarize, SitePage, SiteQuery,
    SiteSort, SiteStatus, SiteSummary, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE,
};
pub use validate::validate;

/// Identifies the document format in storage, exports and imports.
pub const MODEL_VERSION: &str = "pingora.panel.config/v1";
