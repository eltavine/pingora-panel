use crate::EngineCapability;
use panel_errors::{Diagnostic, ErrorCode, PanelError, Result, ValidationReport};
use panel_ir::{RouteAction, RouteMatcher, RuntimeSnapshot, IR_SCHEMA_VERSION};
use std::collections::{btree_map::Entry, BTreeMap, BTreeSet};

/// Validate engine-neutral invariants before any adapter-specific compilation.
pub fn validate_engine_ir(
    snapshot: &RuntimeSnapshot,
    capabilities: &BTreeSet<EngineCapability>,
) -> Result<ValidationReport> {
    let mut diagnostics = Vec::new();
    if snapshot.schema_version != IR_SCHEMA_VERSION {
        diagnostics.push(Diagnostic::error(
            ErrorCode::VALIDATION_FAILED,
            format!("unsupported IR schema {}", snapshot.schema_version),
        ));
    }
    if !snapshot.has_valid_content_hash() {
        diagnostics.push(Diagnostic::error(
            ErrorCode::VALIDATION_FAILED,
            "declared content hash does not match canonical IR",
        ));
    }
    validate_references(snapshot, &mut diagnostics);
    crate::traffic::validate_traffic(snapshot, &mut diagnostics);
    crate::logging::validate_logging(snapshot, &mut diagnostics);
    crate::http::validate_http(snapshot, &mut diagnostics);
    crate::resilience::validate_resilience(snapshot, &mut diagnostics);
    crate::lua::validate_lua(snapshot, &mut diagnostics);
    crate::rewrite::validate_rewrites(snapshot, &mut diagnostics);

    let unsupported: Vec<_> = snapshot
        .required_capabilities()
        .iter()
        .filter(|required| {
            !capabilities.contains(&EngineCapability::new(
                required.name.clone(),
                required.version.clone(),
            ))
        })
        .collect();
    if !unsupported.is_empty() {
        return Err(PanelError::unsupported_capability(format!(
            "unsupported capabilities: {}",
            unsupported
                .iter()
                .map(|item| format!("{}@{}", item.name, item.version))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    Ok(ValidationReport::from_diagnostics(diagnostics))
}

fn validate_references(snapshot: &RuntimeSnapshot, diagnostics: &mut Vec<Diagnostic>) {
    let mut site_domains = BTreeMap::new();
    let mut domain_owners = BTreeMap::new();
    for site in &snapshot.sites {
        if site_domains
            .insert(
                site.id.clone(),
                site.domains
                    .iter()
                    .map(|domain| domain.host.clone())
                    .collect::<BTreeSet<_>>(),
            )
            .is_some()
        {
            diagnostics.push(Diagnostic::error(
                ErrorCode::VALIDATION_FAILED,
                format!("duplicate site id {}", site.id),
            ));
        }
        for domain in &site.domains {
            match domain_owners.entry(domain.host.clone()) {
                Entry::Vacant(entry) => {
                    entry.insert(&site.id);
                }
                Entry::Occupied(entry) => {
                    diagnostics.push(Diagnostic::error(
                        ErrorCode::VALIDATION_FAILED,
                        format!(
                            "domain {} is assigned more than once (sites {} and {})",
                            domain.host,
                            entry.get(),
                            site.id
                        ),
                    ));
                }
            }
        }
    }

    let mut pool_ids = BTreeSet::new();
    for pool in &snapshot.upstream_pools {
        if !pool_ids.insert(pool.id.clone()) {
            diagnostics.push(Diagnostic::error(
                ErrorCode::VALIDATION_FAILED,
                format!("duplicate upstream pool id {}", pool.id),
            ));
        }
        if pool.endpoints.is_empty() {
            diagnostics.push(Diagnostic::error(
                ErrorCode::VALIDATION_FAILED,
                format!("upstream pool {} has no endpoints", pool.id),
            ));
        }
        let mut endpoint_ids = BTreeSet::new();
        for endpoint in &pool.endpoints {
            if !endpoint_ids.insert(endpoint.id.clone()) {
                diagnostics.push(Diagnostic::error(
                    ErrorCode::VALIDATION_FAILED,
                    format!(
                        "upstream pool {} has duplicate endpoint id {}",
                        pool.id, endpoint.id
                    ),
                ));
            }
            if endpoint.weight == 0 {
                diagnostics.push(Diagnostic::error(
                    ErrorCode::VALIDATION_FAILED,
                    format!(
                        "upstream endpoint {} must have a positive weight",
                        endpoint.id
                    ),
                ));
            }
        }
    }

    let mut route_ids = BTreeSet::new();
    for route in &snapshot.routes {
        if !route_ids.insert(route.id.clone()) {
            diagnostics.push(Diagnostic::error(
                ErrorCode::VALIDATION_FAILED,
                format!("duplicate route id {}", route.id),
            ));
        }
        match site_domains.get(&route.site_id) {
            None => diagnostics.push(Diagnostic::error(
                ErrorCode::VALIDATION_FAILED,
                format!(
                    "route {} references unknown site {}",
                    route.id, route.site_id
                ),
            )),
            Some(domains) if domains.is_empty() => diagnostics.push(Diagnostic::error(
                ErrorCode::VALIDATION_FAILED,
                format!(
                    "route {} belongs to site {} with no domains",
                    route.id, route.site_id
                ),
            )),
            Some(domains) => {
                let matcher_host = match &route.matcher {
                    RouteMatcher::Host { host } | RouteMatcher::HostPathPrefix { host, .. } => {
                        Some(host)
                    }
                    _ => None,
                };
                if let Some(host) = matcher_host.filter(|host| {
                    !domains
                        .iter()
                        .any(|domain: &panel_domain::NormalizedHost| domain.matches(host))
                        && !domains.contains(*host)
                }) {
                    diagnostics.push(Diagnostic::error(
                        ErrorCode::VALIDATION_FAILED,
                        format!(
                            "route {} matches host {} outside site {} domains",
                            route.id, host, route.site_id
                        ),
                    ));
                }
            }
        }
        if let RouteAction::Proxy { upstream_pool_id } = &route.action {
            if !pool_ids.contains(upstream_pool_id) {
                diagnostics.push(Diagnostic::error(
                    ErrorCode::VALIDATION_FAILED,
                    format!(
                        "route {} references unknown upstream pool {}",
                        route.id, upstream_pool_id
                    ),
                ));
            }
        }
        if matches!(&route.action, RouteAction::Redirect { status, .. } | RouteAction::Respond { status, .. } if !(100..=599).contains(status))
        {
            diagnostics.push(Diagnostic::error(
                ErrorCode::VALIDATION_FAILED,
                format!("route {} uses an invalid HTTP status", route.id),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_domain::{
        EndpointAddress, EndpointId, NormalizedHost, RevisionId, RouteId, SiteId, UpstreamPoolId,
    };
    use panel_ir::{DomainSpec, RouteSpec, SiteSpec, UpstreamEndpoint, UpstreamPoolSpec};

    fn site(id: &str, host: &str) -> SiteSpec {
        SiteSpec::new(
            SiteId::new(id).unwrap(),
            id,
            vec![DomainSpec::new(NormalizedHost::new(host).unwrap())],
        )
    }

    #[test]
    fn duplicate_domains_fail_validation_before_prepare() {
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        snapshot.sites = vec![site("first", "Example.COM."), site("second", "example.com")];
        snapshot.refresh_content_hash();
        let report = validate_engine_ir(&snapshot, &BTreeSet::new()).unwrap();
        assert!(!report.valid);
        assert!(report.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("domain example.com is assigned more than once")));

        snapshot.sites.push(site("third", "EXAMPLE.COM"));
        snapshot.refresh_content_hash();
        let report = validate_engine_ir(&snapshot, &BTreeSet::new()).unwrap();
        assert!(report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("sites first and third")));
        snapshot.sites.pop();

        snapshot.sites[1].domains[0].host = NormalizedHost::new("other.example").unwrap();
        snapshot.refresh_content_hash();
        assert!(
            validate_engine_ir(&snapshot, &BTreeSet::new())
                .unwrap()
                .valid
        );

        let duplicate = snapshot.sites[0].domains[0].clone();
        snapshot.sites[0].domains.push(duplicate);
        snapshot.refresh_content_hash();
        assert!(
            !validate_engine_ir(&snapshot, &BTreeSet::new())
                .unwrap()
                .valid
        );
    }

    #[test]
    fn route_host_must_belong_to_a_declared_site_domain() {
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        snapshot.sites.push(site("site", "example.com"));
        snapshot.routes.push(RouteSpec::new(
            RouteId::new("route").unwrap(),
            SiteId::new("site").unwrap(),
            1,
            RouteMatcher::Host {
                host: NormalizedHost::new("other.example").unwrap(),
            },
            RouteAction::respond(200, None),
        ));
        snapshot.refresh_content_hash();
        let report = validate_engine_ir(&snapshot, &BTreeSet::new()).unwrap();
        assert!(report.diagnostics.iter().any(|diagnostic| diagnostic
            .message
            .contains("host other.example outside site site domains")));

        snapshot.sites[0].domains[0].host = NormalizedHost::new("*.example.com").unwrap();
        snapshot.routes[0].matcher = RouteMatcher::Host {
            host: NormalizedHost::new("api.example.com").unwrap(),
        };
        snapshot.refresh_content_hash();
        assert!(
            validate_engine_ir(&snapshot, &BTreeSet::new())
                .unwrap()
                .valid
        );
        snapshot.sites[0].domains[0].host = NormalizedHost::new("example.com").unwrap();

        snapshot.routes[0].matcher = RouteMatcher::PathPrefix {
            path: panel_domain::PathPrefix::new("/api").unwrap(),
        };
        snapshot.sites[0].domains.clear();
        snapshot.refresh_content_hash();
        let report = validate_engine_ir(&snapshot, &BTreeSet::new()).unwrap();
        assert!(report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("site site with no domains")));
    }

    #[tokio::test]
    async fn duplicate_endpoint_ids_fail_before_prepare_without_changing_active() {
        use crate::{FakeGatewayEngine, GatewayEngine, PrepareRequest};

        let gateway = FakeGatewayEngine::with_default_capabilities();
        let mut snapshot = RuntimeSnapshot::empty(RevisionId::new(1));
        let endpoint = UpstreamEndpoint::new(
            EndpointId::new("origin").unwrap(),
            EndpointAddress::new("127.0.0.1", 8080, false).unwrap(),
        );
        snapshot.upstream_pools.push(UpstreamPoolSpec::new(
            UpstreamPoolId::new("pool").unwrap(),
            "pool",
            vec![endpoint.clone(), endpoint],
        ));
        snapshot.refresh_content_hash();
        let report = gateway.validate(snapshot.clone()).await.unwrap();
        assert!(!report.valid);
        assert!(report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("duplicate endpoint id origin")));
        assert!(gateway.prepare(PrepareRequest { snapshot }).await.is_err());
        let status = gateway.status().await.unwrap();
        assert!(status.active_hash.is_none());
        assert_eq!(status.prepared_count, 0);
    }
}
