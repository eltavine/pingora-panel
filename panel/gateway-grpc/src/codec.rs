use panel_contracts::{common::v1 as common, gateway::v1 as wire};
use panel_domain::{
    ContentHash, EndpointAddress, EndpointId, NormalizedHost, PathPrefix, RevisionId, RouteId,
    SiteId, UpstreamPoolId,
};
use panel_errors::{PanelError, Result};
use panel_ir::{
    CachePolicy, CapabilityRequirement, DomainSpec, HeaderPolicy, ListenerRef, LoadBalancingPolicy,
    LuaPolicy, RetryPolicy, RouteAction, RouteMatcher, RouteSpec, RuntimeSnapshot, SecurityPolicy,
    SiteSpec, StaticContentPolicy, TlsProfile, UpstreamEndpoint, UpstreamPoolSpec,
};
use std::collections::{BTreeMap, BTreeSet};

pub fn decode_snapshot(value: wire::RuntimeSnapshot) -> Result<RuntimeSnapshot> {
    let declared_hash = decode_hash(value.content_hash)?;
    let snapshot = RuntimeSnapshot {
        schema_version: value.schema_version,
        revision_id: RevisionId::new(value.revision_id),
        content_hash: declared_hash,
        listeners: value
            .listeners
            .into_iter()
            .map(decode_listener)
            .collect::<Result<Vec<_>>>()?,
        sites: value
            .sites
            .into_iter()
            .map(decode_site)
            .collect::<Result<Vec<_>>>()?,
        routes: value
            .routes
            .into_iter()
            .map(decode_route)
            .collect::<Result<Vec<_>>>()?,
        upstream_pools: value
            .upstream_pools
            .into_iter()
            .map(decode_upstream_pool)
            .collect::<Result<Vec<_>>>()?,
        tls_profiles: value.tls_profiles.into_iter().map(decode_tls).collect(),
        header_policies: value
            .header_policies
            .into_iter()
            .map(decode_header_policy)
            .collect(),
        static_content: value
            .static_content
            .into_iter()
            .map(decode_static_content)
            .collect(),
        cache_policies: value
            .cache_policies
            .into_iter()
            .map(decode_cache_policy)
            .collect(),
        security_policies: value
            .security_policies
            .into_iter()
            .map(decode_security_policy)
            .collect(),
        lua_policies: value
            .lua_policies
            .into_iter()
            .map(decode_lua_policy)
            .collect(),
        required_capabilities: value
            .required_capabilities
            .into_iter()
            .map(|capability| CapabilityRequirement::new(capability.name, capability.version))
            .collect(),
    };
    if !snapshot.has_valid_content_hash() {
        return Err(PanelError::validation_failed(
            "wire snapshot content hash does not match canonical IR",
        ));
    }
    Ok(snapshot)
}

pub fn encode_snapshot(value: &RuntimeSnapshot) -> wire::RuntimeSnapshot {
    wire::RuntimeSnapshot {
        schema_version: value.schema_version.clone(),
        revision_id: value.revision_id.get(),
        content_hash: Some(encode_hash(&value.content_hash)),
        listeners: value.listeners.iter().map(encode_listener).collect(),
        sites: value.sites.iter().map(encode_site).collect(),
        routes: value.routes.iter().map(encode_route).collect(),
        upstream_pools: value
            .upstream_pools
            .iter()
            .map(encode_upstream_pool)
            .collect(),
        tls_profiles: value.tls_profiles.iter().map(encode_tls).collect(),
        header_policies: value
            .header_policies
            .iter()
            .map(encode_header_policy)
            .collect(),
        static_content: value
            .static_content
            .iter()
            .map(encode_static_content)
            .collect(),
        cache_policies: value
            .cache_policies
            .iter()
            .map(encode_cache_policy)
            .collect(),
        security_policies: value
            .security_policies
            .iter()
            .map(encode_security_policy)
            .collect(),
        lua_policies: value.lua_policies.iter().map(encode_lua_policy).collect(),
        required_capabilities: value
            .required_capabilities
            .iter()
            .map(|capability| common::Capability {
                name: capability.name.clone(),
                version: capability.version.clone(),
            })
            .collect(),
    }
}

pub fn decode_hash(value: Option<common::ContentHash>) -> Result<ContentHash> {
    let value = value.ok_or_else(|| PanelError::invalid_argument("content hash is required"))?;
    if value.algorithm != "sha256" {
        return Err(PanelError::invalid_argument(format!(
            "unsupported content hash algorithm {}",
            value.algorithm
        )));
    }
    ContentHash::from_hex(value.value)
        .map_err(|error| PanelError::invalid_argument(error.to_string()))
}

pub fn encode_hash(value: &ContentHash) -> common::ContentHash {
    common::ContentHash {
        algorithm: "sha256".into(),
        value: value.as_str().into(),
    }
}

fn optional_string(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn decode_listener(value: wire::ListenerRef) -> Result<ListenerRef> {
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

fn encode_listener(value: &ListenerRef) -> wire::ListenerRef {
    wire::ListenerRef {
        id: value.id.clone(),
        address: value.address.clone(),
        tls: value.tls_profile_id.is_some(),
        tls_profile_id: value.tls_profile_id.clone().unwrap_or_default(),
    }
}

fn decode_site(value: wire::SiteSpec) -> Result<SiteSpec> {
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

fn encode_site(value: &SiteSpec) -> wire::SiteSpec {
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

fn decode_route(value: wire::RouteSpec) -> Result<RouteSpec> {
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

fn encode_route(value: &RouteSpec) -> wire::RouteSpec {
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

fn decode_upstream_pool(value: wire::UpstreamPoolSpec) -> Result<UpstreamPoolSpec> {
    let load_balancing = if let Some(policy) = value.load_balancing_v1 {
        decode_load_balancing(policy)?
    } else {
        decode_legacy_load_balancing(&value.load_balancing_policy)?
    };
    let retry_policy = if let Some(policy) = value.retry_policy_v1 {
        decode_retry_policy(policy)?
    } else if value.retry_policy.is_empty() || value.retry_policy == "none" {
        RetryPolicy {
            attempts: 0,
            per_try_timeout_ms: 0,
            retry_statuses: BTreeSet::new(),
        }
    } else {
        return Err(PanelError::invalid_argument(
            "legacy retry_policy only supports the value 'none'",
        ));
    };
    Ok(UpstreamPoolSpec {
        id: UpstreamPoolId::new(value.id).map_err(domain_error)?,
        name: value.name,
        endpoints: value
            .endpoints
            .into_iter()
            .map(decode_upstream_endpoint)
            .collect::<Result<Vec<_>>>()?,
        load_balancing,
        retry_policy,
    })
}

fn encode_upstream_pool(value: &UpstreamPoolSpec) -> wire::UpstreamPoolSpec {
    wire::UpstreamPoolSpec {
        id: value.id.as_str().into(),
        name: value.name.clone(),
        endpoints: value
            .endpoints
            .iter()
            .map(encode_upstream_endpoint)
            .collect(),
        load_balancing_policy: String::new(),
        retry_policy: String::new(),
        load_balancing_v1: Some(encode_load_balancing(&value.load_balancing)),
        retry_policy_v1: Some(encode_retry_policy(&value.retry_policy)),
    }
}

fn decode_upstream_endpoint(value: wire::UpstreamEndpoint) -> Result<UpstreamEndpoint> {
    let port = u16::try_from(value.port)
        .map_err(|_| PanelError::invalid_argument("upstream port exceeds 65535"))?;
    Ok(UpstreamEndpoint {
        id: EndpointId::new(value.id).map_err(domain_error)?,
        address: EndpointAddress::new(value.address, port, value.tls).map_err(domain_error)?,
        sni: optional_string(value.sni),
        weight: value.weight,
    })
}

fn encode_upstream_endpoint(value: &UpstreamEndpoint) -> wire::UpstreamEndpoint {
    wire::UpstreamEndpoint {
        id: value.id.as_str().into(),
        address: value.address.host().into(),
        port: u32::from(value.address.port()),
        tls: value.address.tls(),
        sni: value.sni.clone().unwrap_or_default(),
        weight: value.weight,
    }
}

fn decode_load_balancing(value: wire::LoadBalancingPolicy) -> Result<LoadBalancingPolicy> {
    use wire::load_balancing_policy::Kind;
    match value
        .kind
        .ok_or_else(|| PanelError::invalid_argument("load balancing kind is required"))?
    {
        Kind::RoundRobin(_) => Ok(LoadBalancingPolicy::RoundRobin),
        Kind::Random(_) => Ok(LoadBalancingPolicy::Random),
        Kind::ConsistentHashKey(key) => Ok(LoadBalancingPolicy::ConsistentHash { key }),
    }
}

fn decode_legacy_load_balancing(value: &str) -> Result<LoadBalancingPolicy> {
    match value {
        "" | "round_robin" => Ok(LoadBalancingPolicy::RoundRobin),
        "random" => Ok(LoadBalancingPolicy::Random),
        value if value.starts_with("consistent_hash:") => Ok(LoadBalancingPolicy::ConsistentHash {
            key: value["consistent_hash:".len()..].into(),
        }),
        _ => Err(PanelError::invalid_argument(format!(
            "unknown load balancing policy {value}"
        ))),
    }
}

fn encode_load_balancing(value: &LoadBalancingPolicy) -> wire::LoadBalancingPolicy {
    use wire::load_balancing_policy::Kind;
    let kind = match value {
        LoadBalancingPolicy::RoundRobin => Kind::RoundRobin(true),
        LoadBalancingPolicy::Random => Kind::Random(true),
        LoadBalancingPolicy::ConsistentHash { key } => Kind::ConsistentHashKey(key.clone()),
    };
    wire::LoadBalancingPolicy { kind: Some(kind) }
}

fn decode_retry_policy(value: wire::RetryPolicy) -> Result<RetryPolicy> {
    let retry_statuses = value
        .retry_statuses
        .into_iter()
        .map(status_code)
        .collect::<Result<BTreeSet<_>>>()?;
    Ok(RetryPolicy {
        attempts: value.attempts,
        per_try_timeout_ms: value.per_try_timeout_ms,
        retry_statuses,
    })
}

fn encode_retry_policy(value: &RetryPolicy) -> wire::RetryPolicy {
    wire::RetryPolicy {
        attempts: value.attempts,
        per_try_timeout_ms: value.per_try_timeout_ms,
        retry_statuses: value
            .retry_statuses
            .iter()
            .copied()
            .map(u32::from)
            .collect(),
    }
}

fn decode_tls(value: wire::TlsProfile) -> TlsProfile {
    TlsProfile {
        id: value.id,
        certificate_secret_id: value.certificate_ref,
        private_key_secret_id: value.private_key_secret_id,
        min_protocol: value.min_protocol,
        alpn: value.alpn.into_iter().collect(),
    }
}

fn encode_tls(value: &TlsProfile) -> wire::TlsProfile {
    wire::TlsProfile {
        id: value.id.clone(),
        certificate_ref: value.certificate_secret_id.clone(),
        private_key_secret_id: value.private_key_secret_id.clone(),
        min_protocol: value.min_protocol.clone(),
        alpn: value.alpn.iter().cloned().collect(),
    }
}

fn decode_header_policy(value: wire::HeaderPolicy) -> HeaderPolicy {
    let request_set = if value.request_set.is_empty() {
        value.set.into_iter().collect()
    } else {
        value.request_set.into_iter().collect()
    };
    let request_remove = if value.request_remove.is_empty() {
        value.remove.into_iter().collect()
    } else {
        value.request_remove.into_iter().collect()
    };
    HeaderPolicy {
        id: value.id,
        request_set,
        request_remove,
        response_set: value.response_set.into_iter().collect(),
        response_remove: value.response_remove.into_iter().collect(),
    }
}

fn encode_header_policy(value: &HeaderPolicy) -> wire::HeaderPolicy {
    wire::HeaderPolicy {
        id: value.id.clone(),
        set: BTreeMap::new().into_iter().collect(),
        remove: Vec::new(),
        request_set: value.request_set.clone().into_iter().collect(),
        request_remove: value.request_remove.iter().cloned().collect(),
        response_set: value.response_set.clone().into_iter().collect(),
        response_remove: value.response_remove.iter().cloned().collect(),
    }
}

fn decode_static_content(value: wire::StaticContentPolicy) -> StaticContentPolicy {
    StaticContentPolicy {
        id: value.id,
        root: value.root,
        index_files: value.index_files,
        spa_fallback: value.spa_fallback,
    }
}

fn encode_static_content(value: &StaticContentPolicy) -> wire::StaticContentPolicy {
    wire::StaticContentPolicy {
        id: value.id.clone(),
        root: value.root.clone(),
        spa_fallback: value.spa_fallback,
        index_files: value.index_files.clone(),
    }
}

fn decode_cache_policy(value: wire::CachePolicy) -> CachePolicy {
    CachePolicy {
        id: value.id,
        enabled: value.enabled,
        ttl_seconds: value.ttl_seconds,
        vary_headers: value.vary_headers.into_iter().collect(),
    }
}

fn encode_cache_policy(value: &CachePolicy) -> wire::CachePolicy {
    wire::CachePolicy {
        id: value.id.clone(),
        enabled: value.enabled,
        ttl_seconds: value.ttl_seconds,
        vary_headers: value.vary_headers.iter().cloned().collect(),
    }
}

fn decode_security_policy(value: wire::SecurityPolicy) -> SecurityPolicy {
    SecurityPolicy {
        id: value.id,
        allowed_cidrs: value.allowed_cidrs.into_iter().collect(),
        denied_cidrs: value.denied_cidrs.into_iter().collect(),
        request_rate_per_second: value.has_request_rate.then_some(value.request_rate),
    }
}

fn encode_security_policy(value: &SecurityPolicy) -> wire::SecurityPolicy {
    wire::SecurityPolicy {
        id: value.id.clone(),
        allowed_cidrs: value.allowed_cidrs.iter().cloned().collect(),
        request_rate: value.request_rate_per_second.unwrap_or_default(),
        denied_cidrs: value.denied_cidrs.iter().cloned().collect(),
        has_request_rate: value.request_rate_per_second.is_some(),
    }
}

fn decode_lua_policy(value: wire::LuaPolicy) -> LuaPolicy {
    LuaPolicy {
        id: value.id,
        script_secret_id: value.script_ref,
        instruction_limit: value.instruction_limit,
        timeout_ms: value.timeout_ms,
        memory_limit_bytes: value.memory_limit_bytes,
        capabilities: value.capabilities.into_iter().collect(),
    }
}

fn encode_lua_policy(value: &LuaPolicy) -> wire::LuaPolicy {
    wire::LuaPolicy {
        id: value.id.clone(),
        script_ref: value.script_secret_id.clone(),
        capabilities: value.capabilities.iter().cloned().collect(),
        instruction_limit: value.instruction_limit,
        timeout_ms: value.timeout_ms,
        memory_limit_bytes: value.memory_limit_bytes,
    }
}

fn status_code(value: u32) -> Result<u16> {
    let value = u16::try_from(value)
        .map_err(|_| PanelError::invalid_argument("HTTP status exceeds 65535"))?;
    if !(100..=599).contains(&value) {
        return Err(PanelError::invalid_argument(format!(
            "invalid HTTP status {value}"
        )));
    }
    Ok(value)
}

fn domain_error(error: panel_domain::DomainError) -> PanelError {
    PanelError::invalid_argument(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_snapshot_round_trips_without_transport_types_leaking() {
        let snapshot = RuntimeSnapshot::empty(RevisionId::new(7));
        let decoded = decode_snapshot(encode_snapshot(&snapshot)).unwrap();
        assert_eq!(decoded, snapshot);
    }

    #[test]
    fn populated_snapshot_round_trips_additive_v1_fields() {
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(8));
        snapshot.listeners.push(ListenerRef {
            id: "https".into(),
            address: "0.0.0.0:443".into(),
            tls_profile_id: Some("tls-main".into()),
        });
        snapshot.sites.push(SiteSpec {
            id: SiteId::new("site-main").unwrap(),
            name: "main".into(),
            enabled: true,
            domains: vec![DomainSpec {
                host: NormalizedHost::new("example.com").unwrap(),
                tls_profile_id: Some("tls-main".into()),
            }],
        });
        snapshot.routes.push(RouteSpec {
            id: RouteId::new("route-main").unwrap(),
            site_id: SiteId::new("site-main").unwrap(),
            priority: 10,
            enabled: true,
            matcher: RouteMatcher::HostPathPrefix {
                host: NormalizedHost::new("example.com").unwrap(),
                path: PathPrefix::new("/api").unwrap(),
            },
            action: RouteAction::Proxy {
                upstream_pool_id: UpstreamPoolId::new("pool-main").unwrap(),
            },
            retry_policy: Some(RetryPolicy {
                attempts: 2,
                per_try_timeout_ms: 500,
                retry_statuses: [502, 503].into_iter().collect(),
            }),
            header_policy_id: Some("headers".into()),
            cache_policy_id: Some("cache".into()),
            security_policy_id: Some("security".into()),
            lua_policy_id: Some("lua".into()),
        });
        snapshot.upstream_pools.push(UpstreamPoolSpec {
            id: UpstreamPoolId::new("pool-main").unwrap(),
            name: "primary".into(),
            endpoints: vec![UpstreamEndpoint {
                id: EndpointId::new("origin-1").unwrap(),
                address: EndpointAddress::new("127.0.0.1", 8443, true).unwrap(),
                sni: Some("origin.example.com".into()),
                weight: 10,
            }],
            load_balancing: LoadBalancingPolicy::ConsistentHash {
                key: "client-ip".into(),
            },
            retry_policy: RetryPolicy {
                attempts: 3,
                per_try_timeout_ms: 750,
                retry_statuses: [500, 502].into_iter().collect(),
            },
        });
        snapshot.tls_profiles.push(TlsProfile {
            id: "tls-main".into(),
            certificate_secret_id: "cert".into(),
            private_key_secret_id: "key".into(),
            min_protocol: "TLS1.2".into(),
            alpn: ["h2".into(), "http/1.1".into()].into_iter().collect(),
        });
        snapshot.header_policies.push(HeaderPolicy {
            id: "headers".into(),
            request_set: [("x-request".into(), "1".into())].into_iter().collect(),
            request_remove: ["x-remove".into()].into_iter().collect(),
            response_set: [("x-response".into(), "1".into())].into_iter().collect(),
            response_remove: ["server".into()].into_iter().collect(),
        });
        snapshot.static_content.push(StaticContentPolicy {
            id: "static".into(),
            root: "/srv/www".into(),
            index_files: vec!["index.html".into()],
            spa_fallback: true,
        });
        snapshot.cache_policies.push(CachePolicy {
            id: "cache".into(),
            enabled: true,
            ttl_seconds: 60,
            vary_headers: ["accept-encoding".into()].into_iter().collect(),
        });
        snapshot.security_policies.push(SecurityPolicy {
            id: "security".into(),
            allowed_cidrs: ["10.0.0.0/8".into()].into_iter().collect(),
            denied_cidrs: ["10.1.0.0/16".into()].into_iter().collect(),
            request_rate_per_second: Some(100),
        });
        snapshot.lua_policies.push(LuaPolicy {
            id: "lua".into(),
            script_secret_id: "script".into(),
            instruction_limit: 10_000,
            timeout_ms: 10,
            memory_limit_bytes: 1_048_576,
            capabilities: ["request.headers".into()].into_iter().collect(),
        });
        snapshot
            .required_capabilities
            .push(CapabilityRequirement::new("upstream.https", "1"));
        snapshot.refresh_content_hash();

        let decoded = decode_snapshot(encode_snapshot(&snapshot)).unwrap();
        assert_eq!(decoded, snapshot);
    }
}
