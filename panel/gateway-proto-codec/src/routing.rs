//! Listener, site and route wire conversion.

use crate::{
    domain_error, optional_string, status_code,
    upstream::{decode_retry_policy, encode_retry_policy},
};
use panel_contracts::gateway::v1 as wire;
use panel_domain::{NormalizedHost, PathPrefix, RouteId, SiteId, UpstreamPoolId};
use panel_errors::{PanelError, Result};
use panel_ir::{DomainSpec, ListenerRef, RouteAction, RouteMatcher, RouteSpec, SiteSpec};

pub(super) fn decode_listener(value: wire::ListenerRef) -> Result<ListenerRef> {
    if value.tls && value.tls_profile_id.is_empty() {
        return Err(PanelError::invalid_argument(format!(
            "listener {} enables TLS without a tls_profile_id",
            value.id
        )));
    }
    Ok(ListenerRef {
        id: value.id,
        address: value.address,
        tls_profile_id: optional_string(value.tls_profile_id),
    })
}

pub(super) fn encode_listener(value: &ListenerRef) -> wire::ListenerRef {
    wire::ListenerRef {
        id: value.id.clone(),
        address: value.address.clone(),
        tls: value.tls_profile_id.is_some(),
        tls_profile_id: value.tls_profile_id.clone().unwrap_or_default(),
    }
}

pub(super) fn decode_site(value: wire::SiteSpec) -> Result<SiteSpec> {
    Ok(SiteSpec {
        id: SiteId::new(value.id).map_err(domain_error)?,
        name: value.name,
        enabled: value.enabled,
        domains: value
            .domains
            .into_iter()
            .map(|domain| {
                Ok(DomainSpec {
                    host: NormalizedHost::new(domain.host).map_err(domain_error)?,
                    tls_profile_id: optional_string(domain.tls_profile_id),
                })
            })
            .collect::<Result<Vec<_>>>()?,
    })
}

pub(super) fn encode_site(value: &SiteSpec) -> wire::SiteSpec {
    wire::SiteSpec {
        id: value.id.as_str().into(),
        name: value.name.clone(),
        enabled: value.enabled,
        domains: value
            .domains
            .iter()
            .map(|domain| wire::DomainSpec {
                host: domain.host.as_str().into(),
                tls_profile_id: domain.tls_profile_id.clone().unwrap_or_default(),
            })
            .collect(),
    }
}

pub(super) fn decode_route(value: wire::RouteSpec) -> Result<RouteSpec> {
    Ok(RouteSpec {
        id: RouteId::new(value.id).map_err(domain_error)?,
        site_id: SiteId::new(value.site_id).map_err(domain_error)?,
        priority: value.priority,
        enabled: value.enabled,
        matcher: decode_matcher(
            value
                .matcher
                .ok_or_else(|| PanelError::invalid_argument("route matcher is required"))?,
        )?,
        action: decode_action(
            value
                .action
                .ok_or_else(|| PanelError::invalid_argument("route action is required"))?,
        )?,
        retry_policy: value.retry_policy_v1.map(decode_retry_policy).transpose()?,
        header_policy_id: optional_string(value.header_policy_id),
        cache_policy_id: optional_string(value.cache_policy_id),
        security_policy_id: optional_string(value.security_policy_id),
        lua_policy_id: optional_string(value.lua_policy_id),
    })
}

pub(super) fn encode_route(value: &RouteSpec) -> wire::RouteSpec {
    wire::RouteSpec {
        id: value.id.as_str().into(),
        site_id: value.site_id.as_str().into(),
        priority: value.priority,
        enabled: value.enabled,
        matcher: Some(encode_matcher(&value.matcher)),
        action: Some(encode_action(&value.action)),
        retry_policy_v1: value.retry_policy.as_ref().map(encode_retry_policy),
        header_policy_id: value.header_policy_id.clone().unwrap_or_default(),
        cache_policy_id: value.cache_policy_id.clone().unwrap_or_default(),
        security_policy_id: value.security_policy_id.clone().unwrap_or_default(),
        lua_policy_id: value.lua_policy_id.clone().unwrap_or_default(),
    }
}

fn decode_matcher(value: wire::RouteMatcher) -> Result<RouteMatcher> {
    use wire::route_matcher::Kind;
    match value
        .kind
        .ok_or_else(|| PanelError::invalid_argument("route matcher kind is required"))?
    {
        Kind::Host(host) => Ok(RouteMatcher::Host {
            host: NormalizedHost::new(host).map_err(domain_error)?,
        }),
        Kind::PathPrefix(path) => Ok(RouteMatcher::PathPrefix {
            path: PathPrefix::new(path).map_err(domain_error)?,
        }),
        Kind::ExactPath(path) => Ok(RouteMatcher::ExactPath { path }),
        Kind::Glob(pattern) => Ok(RouteMatcher::Glob { pattern }),
        Kind::Regex(pattern) => Ok(RouteMatcher::Regex { pattern }),
        Kind::HostPathPrefix(matcher) => Ok(RouteMatcher::HostPathPrefix {
            host: NormalizedHost::new(matcher.host).map_err(domain_error)?,
            path: PathPrefix::new(matcher.path).map_err(domain_error)?,
        }),
    }
}

fn encode_matcher(value: &RouteMatcher) -> wire::RouteMatcher {
    use wire::route_matcher::Kind;
    let kind = match value {
        RouteMatcher::Host { host } => Kind::Host(host.as_str().into()),
        RouteMatcher::PathPrefix { path } => Kind::PathPrefix(path.as_str().into()),
        RouteMatcher::ExactPath { path } => Kind::ExactPath(path.clone()),
        RouteMatcher::Glob { pattern } => Kind::Glob(pattern.clone()),
        RouteMatcher::Regex { pattern } => Kind::Regex(pattern.clone()),
        RouteMatcher::HostPathPrefix { host, path } => {
            Kind::HostPathPrefix(wire::HostPathPrefixMatcher {
                host: host.as_str().into(),
                path: path.as_str().into(),
            })
        }
    };
    wire::RouteMatcher { kind: Some(kind) }
}

fn decode_action(value: wire::RouteAction) -> Result<RouteAction> {
    use wire::route_action::Kind;
    match value
        .kind
        .ok_or_else(|| PanelError::invalid_argument("route action kind is required"))?
    {
        Kind::UpstreamPoolId(id) => Ok(RouteAction::Proxy {
            upstream_pool_id: UpstreamPoolId::new(id).map_err(domain_error)?,
        }),
        Kind::StaticContentId(policy_id) => Ok(RouteAction::Static { policy_id }),
        Kind::RedirectUrl(location) => Ok(RouteAction::Redirect {
            location,
            status: 302,
        }),
        Kind::ReturnStatus(status) => Ok(RouteAction::Respond {
            status: status_code(status)?,
            body: None,
        }),
        Kind::Redirect(action) => Ok(RouteAction::Redirect {
            location: action.location,
            status: status_code(action.status)?,
        }),
        Kind::Respond(action) => Ok(RouteAction::Respond {
            status: status_code(action.status)?,
            body: action.has_body.then_some(action.body),
        }),
    }
}

fn encode_action(value: &RouteAction) -> wire::RouteAction {
    use wire::route_action::Kind;
    let kind = match value {
        RouteAction::Proxy { upstream_pool_id } => {
            Kind::UpstreamPoolId(upstream_pool_id.as_str().into())
        }
        RouteAction::Static { policy_id } => Kind::StaticContentId(policy_id.clone()),
        RouteAction::Redirect { location, status } => Kind::Redirect(wire::RedirectAction {
            location: location.clone(),
            status: u32::from(*status),
        }),
        RouteAction::Respond { status, body } => Kind::Respond(wire::RespondAction {
            status: u32::from(*status),
            body: body.clone().unwrap_or_default(),
            has_body: body.is_some(),
        }),
    };
    wire::RouteAction { kind: Some(kind) }
}
