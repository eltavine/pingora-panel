//! Listener, site and route wire conversion.

use crate::{
    domain_error, logging, lua, optional_string, status_code,
    upstream::{decode_retry_policy, encode_retry_policy},
};
use panel_contracts::gateway::v1 as wire;
use panel_domain::{NormalizedHost, PathPrefix, RouteId, SiteId, UpstreamPoolId};
use panel_errors::{PanelError, Result};
use panel_ir::{
    DomainSpec, ListenerProtocols, ListenerRef, RealIpHeader, RewriteFlag, RewriteRule,
    RouteAction, RouteCondition, RouteMatcher, RouteSpec, SiteSpec, StrictTransportSecurity,
    ValueTest, WwwRedirect,
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
        trusted_proxies: value.trusted_proxies.into_iter().collect(),
        real_ip_header: match wire::RealIpHeader::try_from(value.real_ip_header) {
            Ok(wire::RealIpHeader::Unspecified | wire::RealIpHeader::XForwardedFor) => {
                RealIpHeader::XForwardedFor
            }
            Ok(wire::RealIpHeader::XRealIp) => RealIpHeader::XRealIp,
            Ok(wire::RealIpHeader::Forwarded) => RealIpHeader::Forwarded,
            Err(_) => {
                return Err(PanelError::invalid_argument(format!(
                    "unknown real IP header {}",
                    value.real_ip_header
                )))
            }
        },
        request_head_timeout_ms: value.request_head_timeout_ms,
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
        trusted_proxies: value.trusted_proxies.iter().cloned().collect(),
        real_ip_header: match value.real_ip_header {
            RealIpHeader::XRealIp => wire::RealIpHeader::XRealIp,
            RealIpHeader::Forwarded => wire::RealIpHeader::Forwarded,
            _ => wire::RealIpHeader::XForwardedFor,
        }
        .into(),
        request_head_timeout_ms: value.request_head_timeout_ms,
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
        security_policy_id: optional_string(value.security_policy_id),
        access_log: logging::decode_access_log(value.access_log)?,
        header_policy_id: optional_string(value.header_policy_id),
        lua: lua::decode_handlers(value.lua)?,
        rewrites: decode_rewrites(value.rewrites)?,
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
        security_policy_id: value.security_policy_id.clone().unwrap_or_default(),
        access_log: logging::encode_access_log(&value.access_log),
        header_policy_id: value.header_policy_id.clone().unwrap_or_default(),
        lua: lua::encode_handlers(&value.lua),
        rewrites: value.rewrites.iter().map(encode_rewrite).collect(),
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
        conditions: value
            .conditions
            .into_iter()
            .map(decode_condition)
            .collect::<Result<_>>()?,
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
        access_log: logging::decode_access_log(value.access_log)?,
        lua: lua::decode_handlers(value.lua)?,
        rewrites: decode_rewrites(value.rewrites)?,
        internal: value.internal,
    })
}

pub(super) fn encode_route(value: &RouteSpec) -> wire::RouteSpec {
    wire::RouteSpec {
        id: value.id.as_str().into(),
        site_id: value.site_id.as_str().into(),
        priority: value.priority,
        enabled: value.enabled,
        matcher: Some(encode_matcher(&value.matcher)),
        conditions: value.conditions.iter().map(encode_condition).collect(),
        action: Some(encode_action(&value.action)),
        retry_policy_v1: value.retry_policy.as_ref().map(encode_retry_policy),
        header_policy_id: value.header_policy_id.clone().unwrap_or_default(),
        cache_policy_id: value.cache_policy_id.clone().unwrap_or_default(),
        security_policy_id: value.security_policy_id.clone().unwrap_or_default(),
        lua_policy_id: value.lua_policy_id.clone().unwrap_or_default(),
        name: value.name.clone().unwrap_or_default(),
        access_log: logging::encode_access_log(&value.access_log),
        lua: lua::encode_handlers(&value.lua),
        rewrites: value.rewrites.iter().map(encode_rewrite).collect(),
        internal: value.internal,
    }
}

fn decode_rewrites(values: Vec<wire::RewriteRule>) -> Result<Vec<RewriteRule>> {
    values.into_iter().map(decode_rewrite).collect()
}

fn decode_rewrite(value: wire::RewriteRule) -> Result<RewriteRule> {
    use wire::rewrite_rule::Kind;
    Ok(
        match value.kind.ok_or_else(|| {
            PanelError::invalid_argument("a rewrite rule is of a kind this gateway does not know")
        })? {
            Kind::StripPrefix(prefix) => RewriteRule::StripPrefix { prefix },
            Kind::AddPrefix(prefix) => RewriteRule::AddPrefix { prefix },
            Kind::SetUri(template) => RewriteRule::SetUri { template },
            Kind::Rewrite(rule) => RewriteRule::Rewrite {
                pattern: rule.pattern,
                replacement: rule.replacement,
                flag: match wire::RewriteFlag::try_from(rule.flag) {
                    Ok(wire::RewriteFlag::Unspecified) => RewriteFlag::None,
                    Ok(wire::RewriteFlag::Last) => RewriteFlag::Last,
                    Ok(wire::RewriteFlag::Break) => RewriteFlag::Break,
                    Ok(wire::RewriteFlag::Redirect) => RewriteFlag::Redirect,
                    Ok(wire::RewriteFlag::Permanent) => RewriteFlag::Permanent,
                    Err(_) => {
                        return Err(PanelError::invalid_argument(format!(
                            "unknown rewrite flag {}",
                            rule.flag
                        )))
                    }
                },
            },
        },
    )
}

fn encode_rewrite(value: &RewriteRule) -> wire::RewriteRule {
    use wire::rewrite_rule::Kind;
    let kind = match value {
        RewriteRule::StripPrefix { prefix } => Kind::StripPrefix(prefix.clone()),
        RewriteRule::AddPrefix { prefix } => Kind::AddPrefix(prefix.clone()),
        RewriteRule::SetUri { template } => Kind::SetUri(template.clone()),
        RewriteRule::Rewrite {
            pattern,
            replacement,
            flag,
        } => Kind::Rewrite(wire::RegexRewrite {
            pattern: pattern.clone(),
            replacement: replacement.clone(),
            flag: match flag {
                RewriteFlag::None => wire::RewriteFlag::Unspecified,
                RewriteFlag::Last => wire::RewriteFlag::Last,
                RewriteFlag::Break => wire::RewriteFlag::Break,
                RewriteFlag::Redirect => wire::RewriteFlag::Redirect,
                RewriteFlag::Permanent => wire::RewriteFlag::Permanent,
            }
            .into(),
        }),
    };
    wire::RewriteRule { kind: Some(kind) }
}

fn decode_condition(value: wire::RouteCondition) -> Result<RouteCondition> {
    use wire::route_condition::Kind;
    let field = |field: wire::FieldCondition| -> Result<(String, ValueTest)> {
        Ok((field.name, decode_test(field.test)?))
    };
    let group = |group: wire::RouteConditions| -> Result<Vec<RouteCondition>> {
        group.conditions.into_iter().map(decode_condition).collect()
    };
    Ok(
        match value.kind.ok_or_else(|| {
            PanelError::invalid_argument(
                "a route condition is of a kind this gateway does not know",
            )
        })? {
            Kind::Method(method) => RouteCondition::Method {
                methods: method.methods,
            },
            Kind::Host(host) => RouteCondition::Host {
                hosts: host
                    .hosts
                    .into_iter()
                    .map(|host| NormalizedHost::new(host).map_err(domain_error))
                    .collect::<Result<_>>()?,
            },
            Kind::Header(header) => {
                let (name, test) = field(header)?;
                RouteCondition::Header { name, test }
            }
            Kind::Query(query) => {
                let (name, test) = field(query)?;
                RouteCondition::Query { name, test }
            }
            Kind::Cookie(cookie) => {
                let (name, test) = field(cookie)?;
                RouteCondition::Cookie { name, test }
            }
            Kind::Client(client) => RouteCondition::Client {
                networks: client.networks,
            },
            Kind::UserAgent(test) => RouteCondition::UserAgent {
                test: decode_test(Some(test))?,
            },
            Kind::Referer(test) => RouteCondition::Referer {
                test: decode_test(Some(test))?,
            },
            Kind::ContentType(content_type) => RouteCondition::ContentType {
                types: content_type.types,
            },
            Kind::All(all) => RouteCondition::All {
                conditions: group(all)?,
            },
            Kind::Any(any) => RouteCondition::Any {
                conditions: group(any)?,
            },
            Kind::Not(condition) => RouteCondition::Not {
                condition: Box::new(decode_condition(*condition)?),
            },
        },
    )
}

fn decode_test(value: Option<wire::ValueTest>) -> Result<ValueTest> {
    use wire::ValueTestOperator as Operator;
    let test = value.ok_or_else(|| PanelError::invalid_argument("a value test is required"))?;
    let (value, ignore_case) = (test.value, test.ignore_case);
    Ok(match Operator::try_from(test.operator) {
        Ok(Operator::Present) => ValueTest::Present,
        Ok(Operator::Absent) => ValueTest::Absent,
        Ok(Operator::Equals) => ValueTest::Equals { value, ignore_case },
        Ok(Operator::Prefix) => ValueTest::Prefix { value, ignore_case },
        Ok(Operator::Suffix) => ValueTest::Suffix { value, ignore_case },
        Ok(Operator::Contains) => ValueTest::Contains { value, ignore_case },
        Ok(Operator::Regex) => ValueTest::Regex {
            pattern: value,
            ignore_case,
        },
        _ => {
            return Err(PanelError::invalid_argument(
                "a value test uses an operator this gateway does not know",
            ))
        }
    })
}

fn encode_condition(value: &RouteCondition) -> wire::RouteCondition {
    use wire::route_condition::Kind;
    let field = |name: &str, test: &ValueTest| wire::FieldCondition {
        name: name.to_owned(),
        test: Some(encode_test(test)),
    };
    let group = |conditions: &[RouteCondition]| wire::RouteConditions {
        conditions: conditions.iter().map(encode_condition).collect(),
    };
    let kind = match value {
        RouteCondition::Method { methods } => Kind::Method(wire::MethodCondition {
            methods: methods.clone(),
        }),
        RouteCondition::Host { hosts } => Kind::Host(wire::HostCondition {
            hosts: hosts.iter().map(|host| host.as_str().to_owned()).collect(),
        }),
        RouteCondition::Header { name, test } => Kind::Header(field(name, test)),
        RouteCondition::Query { name, test } => Kind::Query(field(name, test)),
        RouteCondition::Cookie { name, test } => Kind::Cookie(field(name, test)),
        RouteCondition::Client { networks } => Kind::Client(wire::ClientCondition {
            networks: networks.clone(),
        }),
        RouteCondition::UserAgent { test } => Kind::UserAgent(encode_test(test)),
        RouteCondition::Referer { test } => Kind::Referer(encode_test(test)),
        RouteCondition::ContentType { types } => Kind::ContentType(wire::ContentTypeCondition {
            types: types.clone(),
        }),
        RouteCondition::All { conditions } => Kind::All(group(conditions)),
        RouteCondition::Any { conditions } => Kind::Any(group(conditions)),
        RouteCondition::Not { condition } => Kind::Not(Box::new(encode_condition(condition))),
    };
    wire::RouteCondition { kind: Some(kind) }
}

fn encode_test(value: &ValueTest) -> wire::ValueTest {
    use wire::ValueTestOperator as Operator;
    let (operator, value, ignore_case) = match value {
        ValueTest::Present => (Operator::Present, String::new(), false),
        ValueTest::Absent => (Operator::Absent, String::new(), false),
        ValueTest::Equals { value, ignore_case } => (Operator::Equals, value.clone(), *ignore_case),
        ValueTest::Prefix { value, ignore_case } => (Operator::Prefix, value.clone(), *ignore_case),
        ValueTest::Suffix { value, ignore_case } => (Operator::Suffix, value.clone(), *ignore_case),
        ValueTest::Contains { value, ignore_case } => {
            (Operator::Contains, value.clone(), *ignore_case)
        }
        ValueTest::Regex {
            pattern,
            ignore_case,
        } => (Operator::Regex, pattern.clone(), *ignore_case),
    };
    wire::ValueTest {
        operator: operator as i32,
        value,
        ignore_case,
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
        Kind::Named(name) => Ok(RouteMatcher::Named { name }),
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
        RouteMatcher::Named { name } => Kind::Named(name.clone()),
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
        Kind::Lua(handler) => Ok(RouteAction::Lua {
            handler: lua::decode_handler(handler)?,
        }),
        Kind::InternalRedirect(target) => Ok(RouteAction::InternalRedirect { target }),
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
        RouteAction::Lua { handler } => Kind::Lua(lua::encode_handler(handler)),
        RouteAction::InternalRedirect { target } => Kind::InternalRedirect(target.clone()),
    };
    wire::RouteAction { kind: Some(kind) }
}
