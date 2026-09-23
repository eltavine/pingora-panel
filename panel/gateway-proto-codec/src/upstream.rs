//! Upstream policies, including explicit legacy wire fallbacks.

use crate::{domain_error, optional_string, status_code};
use panel_contracts::gateway::v1 as wire;
use panel_domain::{EndpointAddress, EndpointId, UpstreamPoolId};
use panel_errors::{PanelError, Result};
use panel_ir::{LoadBalancingPolicy, RetryPolicy, UpstreamEndpoint, UpstreamPoolSpec};
use std::collections::BTreeSet;

pub(super) fn decode_upstream_pool(value: wire::UpstreamPoolSpec) -> Result<UpstreamPoolSpec> {
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

pub(super) fn encode_upstream_pool(value: &UpstreamPoolSpec) -> wire::UpstreamPoolSpec {
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

pub(super) fn decode_retry_policy(value: wire::RetryPolicy) -> Result<RetryPolicy> {
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

pub(super) fn encode_retry_policy(value: &RetryPolicy) -> wire::RetryPolicy {
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
