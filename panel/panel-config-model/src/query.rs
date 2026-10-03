//! Read-side views: site status, the site overview and list queries.

use crate::model::{Action, ConfigModel, Site, SiteKind};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::SecondsFormat;
use panel_errors::{Diagnostic, PanelError, Result};
use serde::{Deserialize, Serialize};
use std::{cmp::Ordering, collections::BTreeSet};
use uuid::Uuid;

pub const DEFAULT_PAGE_SIZE: usize = 50;
pub const MAX_PAGE_SIZE: usize = 500;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SiteStatus {
    Running,
    Stopped,
    /// Enabled but unable to serve as configured.
    Abnormal,
    Deleted,
}

/// Sites that are enabled yet cannot serve: their configuration has errors,
/// no enabled domain, or a proxy target without an enabled node.
pub fn abnormal_sites(model: &ConfigModel, diagnostics: &[Diagnostic]) -> BTreeSet<Uuid> {
    let mut abnormal: BTreeSet<Uuid> = diagnostics
        .iter()
        .filter_map(|diagnostic| diagnostic.resource_id.as_deref())
        .filter_map(|resource| resource.strip_prefix("sites/"))
        .filter_map(|rest| rest.split('/').next())
        .filter_map(|id| id.parse().ok())
        .collect();
    for site in model
        .sites
        .iter()
        .filter(|site| site.enabled && !site.is_deleted())
    {
        let serves = site
            .domains
            .iter()
            .any(|domain| domain.enabled && !domain.redirect);
        let targets_ready = std::iter::once(&site.action)
            .chain(
                site.routes
                    .iter()
                    .filter(|route| route.enabled)
                    .map(|route| &route.action),
            )
            .all(|action| match action {
                Action::Proxy { upstream_id } => model
                    .upstreams
                    .iter()
                    .find(|upstream| upstream.id == *upstream_id)
                    .is_some_and(|upstream| upstream.nodes.iter().any(|node| node.enabled)),
                _ => true,
            });
        if !serves || !targets_ready {
            abnormal.insert(site.id);
        }
    }
    abnormal
}

pub fn site_status(site: &Site, abnormal: &BTreeSet<Uuid>) -> SiteStatus {
    if site.is_deleted() {
        SiteStatus::Deleted
    } else if !site.enabled {
        SiteStatus::Stopped
    } else if abnormal.contains(&site.id) {
        SiteStatus::Abnormal
    } else {
        SiteStatus::Running
    }
}

/// Whether some TLS listener serves the site.
pub fn serves_https(model: &ConfigModel, site: &Site) -> bool {
    model.listeners.iter().any(|listener| {
        listener.tls_profile_id.is_some()
            && (site.listener_ids.is_empty() || site.listener_ids.contains(&listener.id))
    })
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SiteSummary {
    /// Sites outside the recycle bin.
    pub total: usize,
    pub running: usize,
    pub stopped: usize,
    pub abnormal: usize,
    pub https: usize,
    pub reverse_proxy: usize,
    #[serde(rename = "static")]
    pub static_sites: usize,
    pub redirect: usize,
    pub maintenance: usize,
    pub deleted: usize,
}

pub fn summarize(model: &ConfigModel, abnormal: &BTreeSet<Uuid>) -> SiteSummary {
    let mut summary = SiteSummary::default();
    for site in &model.sites {
        match site_status(site, abnormal) {
            SiteStatus::Deleted => {
                summary.deleted += 1;
                continue;
            }
            SiteStatus::Running => summary.running += 1,
            SiteStatus::Stopped => summary.stopped += 1,
            SiteStatus::Abnormal => summary.abnormal += 1,
        }
        summary.total += 1;
        if serves_https(model, site) {
            summary.https += 1;
        }
        match site.kind() {
            SiteKind::ReverseProxy => summary.reverse_proxy += 1,
            SiteKind::Static => summary.static_sites += 1,
            SiteKind::Redirect => summary.redirect += 1,
            SiteKind::Maintenance => summary.maintenance += 1,
        }
    }
    summary
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SiteSort {
    #[default]
    Name,
    CreatedAt,
    UpdatedAt,
    Status,
    Domain,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[cfg_attr(
    feature = "openapi",
    derive(utoipa::IntoParams),
    into_params(parameter_in = Query)
)]
#[serde(default)]
pub struct SiteQuery {
    /// Matches names, domains, notes, groups and tags, case-insensitively.
    pub q: Option<String>,
    pub status: Option<SiteStatus>,
    pub kind: Option<SiteKind>,
    /// Matches sites with this domain; wildcards cover one label.
    pub domain: Option<String>,
    pub tag: Option<String>,
    pub group: Option<String>,
    pub favorite: Option<bool>,
    pub sort: SiteSort,
    pub descending: bool,
    pub limit: Option<usize>,
    /// Opaque position returned as `next_cursor` by the previous page.
    pub cursor: Option<String>,
}

pub struct SitePage<'a> {
    pub items: Vec<&'a Site>,
    pub next_cursor: Option<String>,
    /// Sites matching the filters across all pages.
    pub total: usize,
}

#[derive(Serialize, Deserialize)]
struct Cursor {
    key: String,
    id: Uuid,
}

pub fn query_sites<'a>(
    model: &'a ConfigModel,
    abnormal: &BTreeSet<Uuid>,
    query: &SiteQuery,
) -> Result<SitePage<'a>> {
    let limit = query.limit.unwrap_or(DEFAULT_PAGE_SIZE);
    if limit == 0 || limit > MAX_PAGE_SIZE {
        return Err(PanelError::invalid_argument(format!(
            "limit must be between 1 and {MAX_PAGE_SIZE}"
        )));
    }
    let keyword = query
        .q
        .as_deref()
        .map(str::to_lowercase)
        .filter(|q| !q.is_empty());
    let domain = query
        .domain
        .as_deref()
        .map(panel_domain::NormalizedHost::new)
        .transpose()
        .map_err(|error| PanelError::invalid_argument(format!("domain filter: {error}")))?;
    let mut matched: Vec<(String, &Site)> = model
        .sites
        .iter()
        .filter(|site| {
            let status = site_status(site, abnormal);
            match query.status {
                Some(wanted) => status == wanted,
                None => status != SiteStatus::Deleted,
            }
        })
        .filter(|site| query.kind.is_none_or(|kind| site.kind() == kind))
        .filter(|site| {
            query
                .favorite
                .is_none_or(|favorite| site.favorite == favorite)
        })
        .filter(|site| {
            query
                .tag
                .as_ref()
                .is_none_or(|tag| site.tags.contains(tag.as_str()))
        })
        .filter(|site| {
            query
                .group
                .as_ref()
                .is_none_or(|group| site.group.as_deref() == Some(group.as_str()))
        })
        .filter(|site| {
            domain.as_ref().is_none_or(|wanted| {
                site.domains
                    .iter()
                    .any(|domain| domain.host == *wanted || domain.host.matches(wanted))
            })
        })
        .filter(|site| {
            keyword
                .as_ref()
                .is_none_or(|keyword| mentions(site, keyword))
        })
        .map(|site| (sort_key(site, query.sort, abnormal), site))
        .collect();
    let order = |left: &(String, &Site), right: &(String, &Site)| {
        let ordering = left.0.cmp(&right.0).then(left.1.id.cmp(&right.1.id));
        if query.descending {
            ordering.reverse()
        } else {
            ordering
        }
    };
    matched.sort_by(order);
    let total = matched.len();
    let start = match &query.cursor {
        Some(cursor) => {
            let cursor = decode_cursor(cursor)?;
            matched.partition_point(|(key, site)| {
                let ordering = key.as_str().cmp(&cursor.key).then(site.id.cmp(&cursor.id));
                if query.descending {
                    ordering != Ordering::Less
                } else {
                    ordering != Ordering::Greater
                }
            })
        }
        None => 0,
    };
    let page: Vec<_> = matched.iter().skip(start).take(limit).collect();
    let next_cursor = (start + page.len() < total)
        .then(|| page.last())
        .flatten()
        .map(|(key, site)| encode_cursor(key, site.id));
    Ok(SitePage {
        items: page.into_iter().map(|(_, site)| *site).collect(),
        next_cursor,
        total,
    })
}

fn mentions(site: &Site, keyword: &str) -> bool {
    site.name.to_lowercase().contains(keyword)
        || site.domains.iter().any(|domain| {
            domain.host.as_str().contains(keyword) || domain.host.to_unicode().contains(keyword)
        })
        || site
            .note
            .as_deref()
            .is_some_and(|note| note.to_lowercase().contains(keyword))
        || site
            .group
            .as_deref()
            .is_some_and(|group| group.to_lowercase().contains(keyword))
        || site
            .tags
            .iter()
            .any(|tag| tag.to_lowercase().contains(keyword))
}

fn sort_key(site: &Site, sort: SiteSort, abnormal: &BTreeSet<Uuid>) -> String {
    match sort {
        SiteSort::Name => site.name.to_lowercase(),
        SiteSort::CreatedAt => site.created_at.to_rfc3339_opts(SecondsFormat::Micros, true),
        SiteSort::UpdatedAt => site.updated_at.to_rfc3339_opts(SecondsFormat::Micros, true),
        SiteSort::Status => format!("{:?}", site_status(site, abnormal)),
        SiteSort::Domain => site
            .domains
            .first()
            .map(|domain| domain.host.as_str().to_owned())
            .unwrap_or_default(),
    }
}

fn encode_cursor(key: &str, id: Uuid) -> String {
    URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&Cursor {
            key: key.to_owned(),
            id,
        })
        .expect("cursors serialize"),
    )
}

fn decode_cursor(value: &str) -> Result<Cursor> {
    URL_SAFE_NO_PAD
        .decode(value)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .ok_or_else(|| PanelError::invalid_argument("the cursor is not one this API returned"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Domain, Listener, Upstream, UpstreamNode};
    use chrono::{Duration, Utc};
    use panel_domain::NormalizedHost;
    use panel_ir::{ListenerProtocols, LoadBalancingPolicy, WwwRedirect};

    fn site(name: &str, host: &str, action: Action) -> Site {
        Site {
            id: Uuid::now_v7(),
            name: name.into(),
            action,
            enabled: true,
            domains: vec![Domain {
                host: NormalizedHost::new(host).unwrap(),
                enabled: true,
                primary: false,
                redirect: false,
                tls_profile_id: None,
            }],
            routes: Vec::new(),
            listener_ids: BTreeSet::new(),
            https_redirect: false,
            www_redirect: WwwRedirect::None,
            tls_profile_id: None,
            group: None,
            tags: BTreeSet::new(),
            note: None,
            favorite: false,
            deleted_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn fixture() -> ConfigModel {
        let upstream = Upstream {
            id: Uuid::now_v7(),
            name: "app".into(),
            nodes: vec![UpstreamNode {
                id: Uuid::now_v7(),
                host: "127.0.0.1".into(),
                port: 80,
                tls: false,
                weight: 1,
                enabled: true,
                backup: false,
                sni: None,
                unix_socket: None,
                note: None,
            }],
            balancing: LoadBalancingPolicy::RoundRobin,
            host_header: None,
            tls: Default::default(),
            connection: Default::default(),
            health_check: None,
            passive_health: None,
            note: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let maintenance = Action::Respond {
            status: 503,
            body: None,
            content_type: None,
            retry_after_seconds: None,
        };
        let mut proxy = site(
            "Shop",
            "shop.example.com",
            Action::Proxy {
                upstream_id: upstream.id,
            },
        );
        proxy.tags.insert("prod".into());
        proxy.favorite = true;
        proxy.tls_profile_id = Some("main".into());
        let mut stopped = site("blog", "blog.example.com", maintenance.clone());
        stopped.enabled = false;
        stopped.created_at = Utc::now() - Duration::days(1);
        let mut broken = site(
            "api",
            "api.example.com",
            Action::Proxy {
                upstream_id: Uuid::now_v7(),
            },
        );
        broken.group = Some("backend".into());
        let mut deleted = site("old", "old.example.com", maintenance);
        deleted.deleted_at = Some(Utc::now());
        let mut model = ConfigModel {
            listeners: vec![Listener {
                id: "https".into(),
                address: "0.0.0.0:443".into(),
                tls_profile_id: Some("main".into()),
                protocols: ListenerProtocols::default(),
                reuse_port: false,
                ipv6_only: None,
                default_site_id: None,
            }],
            upstreams: vec![upstream],
            tls_profiles: vec![crate::TlsProfile {
                id: "main".into(),
                certificate_id: None,
                certificate_secret_id: "main.crt".into(),
                private_key_secret_id: "main.key".into(),
                min_protocol: "TLSv1.2".into(),
                alpn: BTreeSet::new(),
            }],
            ..ConfigModel::default()
        };
        model.sites = vec![proxy, stopped, broken, deleted];
        model
    }

    #[test]
    fn overview_counts_status_type_and_https() {
        let model = fixture();
        let abnormal = abnormal_sites(&model, &crate::validate::validate(&model));
        let summary = summarize(&model, &abnormal);
        assert_eq!(
            summary,
            SiteSummary {
                total: 3,
                running: 1,
                stopped: 1,
                abnormal: 1,
                https: 3,
                reverse_proxy: 2,
                static_sites: 0,
                redirect: 0,
                maintenance: 1,
                deleted: 1,
            }
        );
    }

    #[test]
    fn filters_combine_and_pages_follow_cursors() {
        let model = fixture();
        let abnormal = abnormal_sites(&model, &crate::validate::validate(&model));
        let names = |query: SiteQuery| -> Vec<String> {
            query_sites(&model, &abnormal, &query)
                .unwrap()
                .items
                .iter()
                .map(|site| site.name.clone())
                .collect()
        };
        assert_eq!(names(SiteQuery::default()), ["api", "blog", "Shop"]);
        assert_eq!(
            names(SiteQuery {
                q: Some("SHOP".into()),
                ..SiteQuery::default()
            }),
            ["Shop"]
        );
        assert_eq!(
            names(SiteQuery {
                status: Some(SiteStatus::Deleted),
                ..SiteQuery::default()
            }),
            ["old"]
        );
        assert_eq!(
            names(SiteQuery {
                kind: Some(SiteKind::Maintenance),
                ..SiteQuery::default()
            }),
            ["blog"]
        );
        assert_eq!(
            names(SiteQuery {
                tag: Some("prod".into()),
                favorite: Some(true),
                ..SiteQuery::default()
            }),
            ["Shop"]
        );
        assert_eq!(
            names(SiteQuery {
                domain: Some("API.example.com".into()),
                ..SiteQuery::default()
            }),
            ["api"]
        );
        assert_eq!(
            names(SiteQuery {
                sort: SiteSort::CreatedAt,
                descending: true,
                ..SiteQuery::default()
            })
            .last()
            .unwrap(),
            "blog"
        );

        let mut seen = Vec::new();
        let mut cursor = None;
        loop {
            let page = query_sites(
                &model,
                &abnormal,
                &SiteQuery {
                    limit: Some(2),
                    cursor: cursor.clone(),
                    descending: true,
                    ..SiteQuery::default()
                },
            )
            .unwrap();
            assert_eq!(page.total, 3);
            seen.extend(page.items.iter().map(|site| site.name.clone()));
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert_eq!(seen, ["Shop", "blog", "api"]);
        assert!(query_sites(
            &model,
            &abnormal,
            &SiteQuery {
                cursor: Some("garbage".into()),
                ..SiteQuery::default()
            }
        )
        .is_err());
    }
}
