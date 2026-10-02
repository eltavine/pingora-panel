//! Upstream policies, including explicit legacy wire fallbacks.

use crate::{domain_error, optional_string, status_code};
use panel_contracts::gateway::v1 as wire;
use panel_domain::{EndpointAddress, EndpointId, UpstreamPoolId};
use panel_errors::{PanelError, Result};
use panel_ir::{
    ActiveHealthCheck, HealthCheckProtocol, LoadBalancingPolicy, PassiveHealthPolicy, RetryPolicy,
    UpstreamConnectionPolicy, UpstreamEndpoint, UpstreamPoolSpec, UpstreamTlsPolicy,
};
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
        connection: value.connection.map(decode_connection).unwrap_or_default(),
        tls: value.tls.map(decode_tls_policy).unwrap_or_default(),
        host_header: optional_string(value.host_header),
        health_check: value.health_check.map(decode_health_check).transpose()?,
        passive_health: value.passive_health.map(|policy| PassiveHealthPolicy {
            failure_threshold: policy.failure_threshold,
            ejection_ms: policy.ejection_ms,
        }),
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
        connection: Some(encode_connection(&value.connection)),
        tls: Some(wire::UpstreamTlsPolicy {
            skip_certificate_verification: !value.tls.verify_certificate,
            skip_hostname_verification: !value.tls.verify_hostname,
            ca_secret_id: value.tls.ca_secret_id.clone().unwrap_or_default(),
            sni: value.tls.sni.clone().unwrap_or_default(),
        }),
        host_header: value.host_header.clone().unwrap_or_default(),
        health_check: value.health_check.as_ref().map(encode_health_check),
        passive_health: value
            .passive_health
            .as_ref()
            .map(|policy| wire::PassiveHealthPolicy {
                failure_threshold: policy.failure_threshold,
                ejection_ms: policy.ejection_ms,
            }),
    }
}

fn decode_connection(value: wire::UpstreamConnectionPolicy) -> UpstreamConnectionPolicy {
    UpstreamConnectionPolicy {
        connect_timeout_ms: value.connect_timeout_ms,
        read_timeout_ms: value.read_timeout_ms,
        write_timeout_ms: value.write_timeout_ms,
        idle_timeout_ms: value.idle_timeout_ms,
        keepalive: !value.disable_keepalive,
        max_connections: value.max_connections,
        http2: value.http2,
    }
}

fn encode_connection(value: &UpstreamConnectionPolicy) -> wire::UpstreamConnectionPolicy {
    wire::UpstreamConnectionPolicy {
        connect_timeout_ms: value.connect_timeout_ms,
        read_timeout_ms: value.read_timeout_ms,
        write_timeout_ms: value.write_timeout_ms,
        idle_timeout_ms: value.idle_timeout_ms,
        disable_keepalive: !value.keepalive,
        max_connections: value.max_connections,
        http2: value.http2,
    }
}

fn decode_tls_policy(value: wire::UpstreamTlsPolicy) -> UpstreamTlsPolicy {
    UpstreamTlsPolicy {
        verify_certificate: !value.skip_certificate_verification,
        verify_hostname: !value.skip_hostname_verification,
        ca_secret_id: optional_string(value.ca_secret_id),
        sni: optional_string(value.sni),
    }
}

fn decode_health_check(value: wire::ActiveHealthCheck) -> Result<ActiveHealthCheck> {
    let protocol = match wire::HealthCheckProtocol::try_from(value.protocol) {
        Ok(wire::HealthCheckProtocol::Http) => HealthCheckProtocol::Http,
        Ok(wire::HealthCheckProtocol::Tcp) => HealthCheckProtocol::Tcp,
        Ok(wire::HealthCheckProtocol::Unspecified) | Err(_) => {
            return Err(PanelError::invalid_argument(
                "health check protocol must be HTTP or TCP",
            ))
        }
    };
    Ok(ActiveHealthCheck {
        protocol,
        path: value.path,
        method: value.method,
        interval_ms: value.interval_ms,
        timeout_ms: value.timeout_ms,
        healthy_threshold: value.healthy_threshold,
        unhealthy_threshold: value.unhealthy_threshold,
        expected_statuses: value
            .expected_statuses
            .into_iter()
            .map(status_code)
            .collect::<Result<BTreeSet<_>>>()?,
        host: optional_string(value.host),
    })
}

fn encode_health_check(value: &ActiveHealthCheck) -> wire::ActiveHealthCheck {
    let protocol = match value.protocol {
        HealthCheckProtocol::Http => wire::HealthCheckProtocol::Http,
        HealthCheckProtocol::Tcp => wire::HealthCheckProtocol::Tcp,
    };
    wire::ActiveHealthCheck {
        protocol: protocol.into(),
        path: value.path.clone(),
        method: value.method.clone(),
        interval_ms: value.interval_ms,
        timeout_ms: value.timeout_ms,
        healthy_threshold: value.healthy_threshold,
        unhealthy_threshold: value.unhealthy_threshold,
        expected_statuses: value
            .expected_statuses
            .iter()
            .copied()
            .map(u32::from)
            .collect(),
        host: value.host.clone().unwrap_or_default(),
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
        enabled: !value.disabled,
        backup: value.backup,
        unix_socket: optional_string(value.unix_socket),
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
        disabled: !value.enabled,
        backup: value.backup,
        unix_socket: value.unix_socket.clone().unwrap_or_default(),
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
