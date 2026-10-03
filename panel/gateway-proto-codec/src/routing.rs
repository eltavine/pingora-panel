//! Listener, site and route wire conversion.

use crate::{
    domain_error, optional_string, status_code,
    upstream::{decode_retry_policy, encode_retry_policy},
};
use panel_contracts::gateway::v1 as wire;
use panel_domain::{NormalizedHost, PathPrefix, RouteId, SiteId, UpstreamPoolId};
use panel_errors::{PanelError, Result};
use panel_ir::{
    DomainSpec, ListenerProtocols, ListenerRef, RouteAction, RouteMatcher, RouteSpec, SiteSpec,
    StrictTransportSecurity, WwwRedirect,
};

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
        protocols: value
            .protocols
            .map_or_else(ListenerProtocols::default, |protocols| ListenerProtocols {
                http1: protocols.http1,
                http2: protocols.http2,
                http3: protocols.http3,
            }),
        reuse_port: value.reuse_port,
        ipv6_only: value.ipv6_only,
        default_site_id: optional_string(value.default_site_id)
            .map(SiteId::new)
            .transpose()
            .map_err(domain_error)?,
    })
}

pub(super) fn encode_listener(value: &ListenerRef) -> wire::ListenerRef {
    wire::ListenerRef {
        id: value.id.clone(),
        address: value.address.clone(),
        tls: value.tls_profile_id.is_some(),
        tls_profile_id: value.tls_profile_id.clone().unwrap_or_default(),
        protocols: Some(wire::ListenerProtocols {
            http1: value.protocols.http1,
            http2: value.protocols.http2,
            http3: value.protocols.http3,
        }),
        reuse_port: value.reuse_port,
        ipv6_only: value.ipv6_only,
        default_site_id: value
            .default_site_id
            .as_ref()
            .map(|id| id.as_str().into())
            .unwrap_or_default(),
    }
}

pub(super) fn decode_site(value: wire::SiteSpec) -> Result<SiteSpec> {
    let www_redirect = match wire::WwwRedirect::try_from(value.www_redirect) {
        Ok(wire::WwwRedirect::Unspecified) => WwwRedirect::None,
        Ok(wire::WwwRedirect::Add) => WwwRedirect::AddWww,
        Ok(wire::WwwRedirect::Remove) => WwwRedirect::RemoveWww,
        Err(_) => {
            return Err(PanelError::invalid_argument(format!(
                "unknown www redirect {}",
                value.www_redirect
            )))
        }
    };
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
                    enabled: !domain.disabled,
                    primary: domain.primary,
                    redirect_to_primary: domain.redirect_to_primary,
                })
            })
            .collect::<Result<Vec<_>>>()?,
        listener_ids: value.listener_ids.into_iter().collect(),
        https_redirect: value.https_redirect,
        www_redirect,
        hsts: value.hsts.map(|hsts| StrictTransportSecurity {
            max_age_seconds: hsts.max_age_seconds,
            include_subdomains: hsts.include_subdomains,
            preload: hsts.preload,
        }),
    })
}

pub(super) fn encode_site(value: &SiteSpec) -> wire::SiteSpec {
    let www_redirect = match value.www_redirect {
        WwwRedirect::None => wire::WwwRedirect::Unspecified,
        WwwRedirect::AddWww => wire::WwwRedirect::Add,
        WwwRedirect::RemoveWww => wire::WwwRedirect::Remove,
    };
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
                disabled: !domain.enabled,
                primary: domain.primary,
                redirect_to_primary: domain.redirect_to_primary,
            })
            .collect(),
        listener_ids: value.listener_ids.iter().cloned().collect(),
        https_redirect: value.https_redirect,
        www_redirect: www_redirect.into(),
        hsts: value.hsts.map(|hsts| wire::StrictTransportSecurity {
            max_age_seconds: hsts.max_age_seconds,
            include_subdomains: hsts.include_subdomains,
            preload: hsts.preload,
        }),
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
        name: optional_string(value.name),
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
        name: value.name.clone().unwrap_or_default(),
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
        Kind::RedirectUrl(location) => Ok(RouteAction::redirect(location, 302)),
        Kind::ReturnStatus(status) => Ok(RouteAction::respond(status_code(status)?, None)),
        Kind::Redirect(action) => Ok(RouteAction::Redirect {
            location: action.location,
            status: status_code(action.status)?,
            preserve_path: action.preserve_path,
        }),
        Kind::Respond(action) => Ok(RouteAction::Respond {
            status: status_code(action.status)?,
            body: action.has_body.then_some(action.body),
            content_type: optional_string(action.content_type),
            retry_after_seconds: action.retry_after_seconds,
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
        RouteAction::Redirect {
            location,
            status,
            preserve_path,
        } => Kind::Redirect(wire::RedirectAction {
            location: location.clone(),
            status: u32::from(*status),
            preserve_path: *preserve_path,
        }),
        RouteAction::Respond {
            status,
            body,
            content_type,
            retry_after_seconds,
        } => Kind::Respond(wire::RespondAction {
            status: u32::from(*status),
            body: body.clone().unwrap_or_default(),
            has_body: body.is_some(),
            content_type: content_type.clone().unwrap_or_default(),
            retry_after_seconds: *retry_after_seconds,
        }),
    };
    wire::RouteAction { kind: Some(kind) }
}
