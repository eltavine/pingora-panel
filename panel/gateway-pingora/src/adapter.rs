use crate::{ADAPTER_VERSION, PINGORA_PACKAGE_VERSION};
use arc_swap::ArcSwapOption;
use async_trait::async_trait;
use panel_engine::{validate_engine_ir, DataPlaneAdapter, EngineCapabilities, EngineCapability};
use panel_errors::{Diagnostic, ErrorCode, PanelError, Result, ValidationReport};
use panel_ir::{LoadBalancingPolicy, RouteAction, RouteMatcher, RuntimeSnapshot};
use pingora_core::upstreams::peer::HttpPeer;
use pingora_http::RequestHeader;
use pingora_load_balancing::Backend;
use std::{collections::BTreeSet, net::SocketAddr, sync::Arc};

pub struct PingoraGatewayAdapter {
    active: ArcSwapOption<PreparedPingoraSnapshot>,
}

/// Opaque immutable artifact built entirely before activation.
///
/// Its fields remain private so no upstream Pingora value can cross the adapter
/// boundary even though the associated type is visible to the generic runtime.
pub struct PreparedPingoraSnapshot {
    snapshot: RuntimeSnapshot,
    _peers: Vec<PrivatePeer>,
}

impl Default for PingoraGatewayAdapter {
    fn default() -> Self {
        Self::new()
    }
}

impl PingoraGatewayAdapter {
    pub fn new() -> Self {
        Self {
            active: ArcSwapOption::empty(),
        }
    }

    pub fn pingora_package_version(&self) -> &'static str {
        PINGORA_PACKAGE_VERSION
    }

    pub fn adapter_version(&self) -> &'static str {
        ADAPTER_VERSION
    }

    pub fn active_snapshot(&self) -> Option<RuntimeSnapshot> {
        self.active
            .load_full()
            .map(|prepared| prepared.snapshot.clone())
    }

    fn supported_capabilities() -> BTreeSet<EngineCapability> {
        [
            EngineCapability::new("route.host", "1"),
            EngineCapability::new("route.path-prefix", "1"),
            EngineCapability::new("upstream.http", "1"),
            EngineCapability::new("upstream.https", "1"),
            EngineCapability::new("activation.cas", "1"),
        ]
        .into_iter()
        .collect()
    }

    fn validate_supported_ir(snapshot: &RuntimeSnapshot) -> Result<ValidationReport> {
        let mut unsupported = Vec::new();
        if !snapshot.listeners.is_empty() {
            unsupported.push("listeners");
        }
        if !snapshot.tls_profiles.is_empty() {
            unsupported.push("tls_profiles");
        }
        if !snapshot.header_policies.is_empty() {
            unsupported.push("header_policies");
        }
        if !snapshot.static_content.is_empty() {
            unsupported.push("static_content");
        }
        if !snapshot.cache_policies.is_empty() {
            unsupported.push("cache_policies");
        }
        if !snapshot.security_policies.is_empty() {
            unsupported.push("security_policies");
        }
        if !snapshot.lua_policies.is_empty() {
            unsupported.push("lua_policies");
        }
        for route in &snapshot.routes {
            if !matches!(
                route.matcher,
                RouteMatcher::Host { .. }
                    | RouteMatcher::PathPrefix { .. }
                    | RouteMatcher::HostPathPrefix { .. }
            ) {
                unsupported.push("route matcher");
            }
            if !matches!(route.action, RouteAction::Proxy { .. }) {
                unsupported.push("route action");
            }
            if route.retry_policy.is_some()
                || route.header_policy_id.is_some()
                || route.cache_policy_id.is_some()
                || route.security_policy_id.is_some()
                || route.lua_policy_id.is_some()
            {
                unsupported.push("route policy");
            }
        }
        for pool in &snapshot.upstream_pools {
            if !matches!(pool.load_balancing, LoadBalancingPolicy::RoundRobin) {
                unsupported.push("load balancing policy");
            }
            if pool.retry_policy.attempts != 0 {
                unsupported.push("upstream retry policy");
            }
            if pool.endpoints.is_empty() {
                return Ok(ValidationReport::from_diagnostics(vec![Diagnostic::error(
                    ErrorCode::VALIDATION_FAILED,
                    format!("upstream pool {} has no endpoints", pool.id),
                )]));
            }
            for endpoint in &pool.endpoints {
                if endpoint.weight == 0 {
                    return Ok(ValidationReport::from_diagnostics(vec![Diagnostic::error(
                        ErrorCode::VALIDATION_FAILED,
                        format!(
                            "upstream endpoint {} must have a positive weight",
                            endpoint.id
                        ),
                    )]));
                }
                let _peer = PrivatePeer::try_from(endpoint)?;
            }
        }
        if !unsupported.is_empty() {
            unsupported.sort_unstable();
            unsupported.dedup();
            return Err(PanelError::unsupported_capability(format!(
                "Pingora adapter does not support IR nodes: {}",
                unsupported.join(", ")
            )));
        }
        Ok(ValidationReport::valid())
    }

    fn compile(snapshot: RuntimeSnapshot) -> Result<PreparedPingoraSnapshot> {
        let peers = snapshot
            .upstream_pools
            .iter()
            .flat_map(|pool| pool.endpoints.iter())
            .map(PrivatePeer::try_from)
            .collect::<Result<Vec<_>>>()?;
        Ok(PreparedPingoraSnapshot {
            snapshot,
            _peers: peers,
        })
    }
}

/// All direct interaction with Pingora values remains behind this private descriptor.
struct PrivatePeer {
    _peer: HttpPeer,
    _backend: Backend,
    _header_probe: RequestHeader,
}

impl TryFrom<&panel_ir::UpstreamEndpoint> for PrivatePeer {
    type Error = PanelError;

    fn try_from(endpoint: &panel_ir::UpstreamEndpoint) -> Result<Self> {
        let socket_text = if endpoint.address.host().contains(':') {
            format!("[{}]:{}", endpoint.address.host(), endpoint.address.port())
        } else {
            format!("{}:{}", endpoint.address.host(), endpoint.address.port())
        };
        let socket: SocketAddr = socket_text.parse().map_err(|_| {
            PanelError::invalid_argument(format!(
                "upstream {} must use an IP literal during the initial adapter stage",
                endpoint.id
            ))
        })?;
        let sni = endpoint
            .sni
            .clone()
            .unwrap_or_else(|| endpoint.address.host().to_string());
        let peer = HttpPeer::new(socket, endpoint.address.tls(), sni);
        let backend = Backend::new_with_weight(&socket.to_string(), endpoint.weight as usize)
            .map_err(|error| {
                PanelError::invalid_argument(format!(
                    "invalid Pingora backend for {}: {error}",
                    endpoint.id
                ))
            })?;
        let header_probe = RequestHeader::build("GET", b"/", None).map_err(|error| {
            PanelError::internal(format!("Pingora HTTP header mapping failed: {error}"))
        })?;
        Ok(Self {
            _peer: peer,
            _backend: backend,
            _header_probe: header_probe,
        })
    }
}

#[async_trait]
impl DataPlaneAdapter for PingoraGatewayAdapter {
    type Prepared = PreparedPingoraSnapshot;

    async fn capabilities(&self) -> Result<EngineCapabilities> {
        Ok(EngineCapabilities {
            protocol_version: "pingora.panel.gateway.v1".into(),
            build_version: PINGORA_PACKAGE_VERSION.into(),
            schema_version: panel_ir::IR_SCHEMA_VERSION.into(),
            adapter_version: ADAPTER_VERSION.into(),
            capabilities: Self::supported_capabilities(),
        })
    }

    async fn validate(&self, snapshot: &RuntimeSnapshot) -> Result<ValidationReport> {
        let base = validate_engine_ir(snapshot, &Self::supported_capabilities())?;
        if !base.valid {
            return Ok(base);
        }
        Self::validate_supported_ir(snapshot)
    }

    async fn prepare(&self, snapshot: RuntimeSnapshot) -> Result<Self::Prepared> {
        let report = self.validate(&snapshot).await?;
        if !report.valid {
            return Err(PanelError::new(
                ErrorCode::VALIDATION_FAILED,
                "snapshot validation failed",
            )
            .with_diagnostics(report.diagnostics));
        }
        Self::compile(snapshot)
    }

    fn activate(&self, prepared: Arc<Self::Prepared>) {
        self.active.store(Some(prepared));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_domain::{EndpointAddress, EndpointId, RevisionId, UpstreamPoolId};
    use panel_ir::{
        CachePolicy, CapabilityRequirement, RetryPolicy, UpstreamEndpoint, UpstreamPoolSpec,
    };

    fn mapped_snapshot(tls: bool) -> RuntimeSnapshot {
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        snapshot.upstream_pools.push(UpstreamPoolSpec {
            id: UpstreamPoolId::new("primary").unwrap(),
            name: "primary".into(),
            endpoints: vec![UpstreamEndpoint {
                id: EndpointId::new("local").unwrap(),
                address: EndpointAddress::new("127.0.0.1", if tls { 443 } else { 80 }, tls)
                    .unwrap(),
                sni: tls.then(|| "example.com".into()),
                weight: 1,
            }],
            load_balancing: LoadBalancingPolicy::RoundRobin,
            retry_policy: RetryPolicy {
                attempts: 0,
                per_try_timeout_ms: 0,
                retry_statuses: BTreeSet::new(),
            },
        });
        snapshot
            .required_capabilities
            .push(CapabilityRequirement::new(
                if tls {
                    "upstream.https"
                } else {
                    "upstream.http"
                },
                "1",
            ));
        snapshot.refresh_content_hash();
        snapshot
    }

    #[tokio::test]
    async fn maps_http_and_https_peers_without_listener() {
        let adapter = PingoraGatewayAdapter::new();
        assert!(
            adapter
                .validate(&mapped_snapshot(false))
                .await
                .unwrap()
                .valid
        );
        assert!(
            adapter
                .validate(&mapped_snapshot(true))
                .await
                .unwrap()
                .valid
        );
    }

    #[tokio::test]
    async fn unsupported_nodes_fail_explicitly() {
        let adapter = PingoraGatewayAdapter::new();
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        snapshot.cache_policies.push(CachePolicy {
            id: "cache".into(),
            enabled: true,
            ttl_seconds: 60,
            vary_headers: BTreeSet::new(),
        });
        snapshot.refresh_content_hash();
        let error = adapter.validate(&snapshot).await.unwrap_err();
        assert_eq!(error.code.as_str(), ErrorCode::UNSUPPORTED_CAPABILITY);
    }

    /// Reserved feature gates must never widen the advertised capability set,
    /// so the adapter reports exactly these capabilities under every selection.
    #[tokio::test]
    async fn reserved_feature_gates_do_not_widen_advertised_capabilities() {
        let advertised = PingoraGatewayAdapter::new()
            .capabilities()
            .await
            .unwrap()
            .capabilities;

        assert_eq!(
            advertised,
            [
                EngineCapability::new("activation.cas", "1"),
                EngineCapability::new("route.host", "1"),
                EngineCapability::new("route.path-prefix", "1"),
                EngineCapability::new("upstream.http", "1"),
                EngineCapability::new("upstream.https", "1"),
            ]
            .into_iter()
            .collect::<BTreeSet<_>>()
        );
    }

    #[tokio::test]
    async fn version_and_capabilities_are_reported() {
        let adapter = PingoraGatewayAdapter::new();
        let capabilities = adapter.capabilities().await.unwrap();
        assert_eq!(capabilities.build_version, "0.8.0");
        assert_eq!(adapter.pingora_package_version(), "0.8.0");
        assert!(capabilities
            .capabilities
            .contains(&EngineCapability::new("activation.cas", "1")));
        let prepared = Arc::new(adapter.prepare(mapped_snapshot(false)).await.unwrap());
        adapter.activate(prepared);
        assert!(adapter.active_snapshot().is_some());
    }
}
