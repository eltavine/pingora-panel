//! Read representations shared by the service that produces them and the
//! HTTP contract that documents them.

use crate::TlsProfile;
use crate::{
    model::{ConfigModel, Domain, Listener, Route, Site, SiteKind, Upstream},
    query::{serves_https, site_status, SiteStatus},
};
use panel_domain::NormalizedHost;
use panel_errors::Diagnostic;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

/// A site with the state derived from the rest of the configuration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SiteView {
    #[serde(flatten)]
    pub site: Site,
    pub kind: SiteKind,
    pub status: SiteStatus,
    /// Whether an HTTPS listener serves the site.
    pub https: bool,
    /// Unicode forms of internationalized domains, keyed by their ASCII form.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unicode_hosts: BTreeMap<String, String>,
    pub etag: String,
}

impl SiteView {
    pub fn new(model: &ConfigModel, site: &Site, abnormal: &BTreeSet<Uuid>) -> Self {
        Self {
            kind: site.kind(),
            status: site_status(site, abnormal),
            https: serves_https(model, site),
            unicode_hosts: unicode_hosts(&site.domains),
            etag: crate::entity_tag(site),
            site: site.clone(),
        }
    }
}

fn unicode_hosts(domains: &[Domain]) -> BTreeMap<String, String> {
    domains
        .iter()
        .filter_map(|domain| {
            let unicode = domain.host.to_unicode();
            (unicode != domain.host.as_str()).then(|| (domain.host.as_str().to_owned(), unicode))
        })
        .collect()
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SiteList {
    pub items: Vec<SiteView>,
    /// Pass back as `cursor` for the next page; absent on the last page.
    pub next_cursor: Option<String>,
    /// Matching sites across all pages.
    pub total: usize,
}

/// A domain with the site it belongs to.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DomainView {
    #[serde(flatten)]
    pub domain: Domain,
    pub unicode_host: String,
    pub site_id: Uuid,
    pub site_name: String,
}

impl DomainView {
    pub fn all(model: &ConfigModel) -> Vec<Self> {
        model
            .sites
            .iter()
            .filter(|site| !site.is_deleted())
            .flat_map(|site| {
                site.domains.iter().map(|domain| Self {
                    unicode_host: domain.host.to_unicode(),
                    domain: domain.clone(),
                    site_id: site.id,
                    site_name: site.name.clone(),
                })
            })
            .collect()
    }
}

/// Syntax, normalization and ownership of a host an operator typed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DomainCheck {
    pub input: String,
    /// The canonical ASCII form, when the input is valid.
    pub host: Option<String>,
    pub unicode_host: Option<String>,
    pub wildcard: bool,
    /// Why the input is not a valid host name.
    pub error: Option<String>,
    /// The site already serving this host, if any.
    pub owner: Option<DomainOwner>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DomainOwner {
    pub site_id: Uuid,
    pub site_name: String,
}

impl DomainCheck {
    pub fn new(model: &ConfigModel, input: &str) -> Self {
        match NormalizedHost::new(input) {
            Ok(host) => Self {
                input: input.to_owned(),
                unicode_host: Some(host.to_unicode()),
                wildcard: host.is_wildcard(),
                error: None,
                owner: model
                    .sites
                    .iter()
                    .filter(|site| !site.is_deleted())
                    .find(|site| site.domains.iter().any(|domain| domain.host == host))
                    .map(|site| DomainOwner {
                        site_id: site.id,
                        site_name: site.name.clone(),
                    }),
                host: Some(host.as_str().to_owned()),
            },
            Err(error) => Self {
                input: input.to_owned(),
                host: None,
                unicode_host: None,
                wildcard: false,
                error: Some(error.to_string()),
                owner: None,
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RouteView {
    #[serde(flatten)]
    pub route: Route,
    pub site_id: Uuid,
    pub etag: String,
}

impl RouteView {
    pub fn new(site_id: Uuid, route: &Route) -> Self {
        Self {
            etag: crate::entity_tag(route),
            route: route.clone(),
            site_id,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ListenerView {
    #[serde(flatten)]
    pub listener: Listener,
    pub etag: String,
}

impl ListenerView {
    pub fn new(listener: &Listener) -> Self {
        Self {
            etag: crate::entity_tag(listener),
            listener: listener.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TlsProfileView {
    #[serde(flatten)]
    pub profile: TlsProfile,
    pub etag: String,
}

impl TlsProfileView {
    pub fn new(profile: &TlsProfile) -> Self {
        Self {
            etag: crate::entity_tag(profile),
            profile: profile.clone(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UpstreamView {
    #[serde(flatten)]
    pub upstream: Upstream,
    /// Live sites that send traffic here.
    pub used_by: Vec<Uuid>,
    pub etag: String,
}

impl UpstreamView {
    pub fn new(model: &ConfigModel, upstream: &Upstream) -> Self {
        let used_by = model
            .sites
            .iter()
            .filter(|site| !site.is_deleted())
            .filter(|site| {
                std::iter::once(&site.action)
                    .chain(site.routes.iter().map(|route| &route.action))
                    .any(|action| {
                        matches!(action, crate::Action::Proxy { upstream_id } if *upstream_id == upstream.id)
                    })
            })
            .map(|site| site.id)
            .collect();
        Self {
            etag: crate::entity_tag(upstream),
            upstream: upstream.clone(),
            used_by,
        }
    }
}

/// Diagnostics of the whole draft or of the requested sites.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ValidationResult {
    pub valid: bool,
    #[cfg_attr(feature = "openapi", schema(value_type = Vec<Object>))]
    pub diagnostics: Vec<Diagnostic>,
}

/// One item of a batch change; the batch applies only if every item can.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum BatchAction {
    Enable,
    Disable,
    Delete,
    Restore,
    Purge,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct BatchRequest {
    pub action: BatchAction,
    pub ids: Vec<Uuid>,
}
