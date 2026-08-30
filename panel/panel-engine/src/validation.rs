use crate::EngineCapability;
use panel_errors::{Diagnostic, ErrorCode, PanelError, Result, ValidationReport};
use panel_ir::{RouteAction, RuntimeSnapshot, IR_SCHEMA_VERSION};
use std::collections::BTreeSet;

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
    let mut site_ids = BTreeSet::new();
    for site in &snapshot.sites {
        if !site_ids.insert(site.id.clone()) {
            diagnostics.push(Diagnostic::error(
                ErrorCode::VALIDATION_FAILED,
                format!("duplicate site id {}", site.id),
            ));
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
        for endpoint in &pool.endpoints {
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
        if !site_ids.contains(&route.site_id) {
            diagnostics.push(Diagnostic::error(
                ErrorCode::VALIDATION_FAILED,
                format!(
                    "route {} references unknown site {}",
                    route.id, route.site_id
                ),
            ));
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
