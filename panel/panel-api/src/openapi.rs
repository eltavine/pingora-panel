//! Public HTTP schema assembled from endpoint definitions and shared conventions.

use crate::contract::*;
use conventions::HttpConventions;
use utoipa::OpenApi;

mod conventions;
#[cfg(test)]
mod tests;

#[derive(OpenApi)]
#[openapi(
    info(title = "Pingora Panel API", version = "v1"),
    paths(
        crate::routes::validate, crate::routes::prepare, crate::routes::activate, crate::routes::abort,
        crate::routes::status, crate::routes::receipt, crate::routes::services, crate::routes::openapi,
        crate::configuration::list_sites, crate::configuration::create_site, crate::configuration::site_summary,
        crate::configuration::export_sites, crate::configuration::import_sites, crate::configuration::batch_sites,
        crate::configuration::get_site, crate::configuration::replace_site, crate::configuration::delete_site,
        crate::configuration::enable_site, crate::configuration::disable_site, crate::configuration::favorite_site,
        crate::configuration::unfavorite_site, crate::configuration::restore_site, crate::configuration::clone_site,
        crate::configuration::add_domains, crate::configuration::replace_domain, crate::configuration::remove_domain,
        crate::configuration::list_routes, crate::configuration::create_route, crate::configuration::reorder_routes,
        crate::configuration::get_route, crate::configuration::replace_route, crate::configuration::delete_route,
        crate::configuration::list_domains, crate::configuration::check_domains,
        crate::configuration::list_upstreams, crate::configuration::create_upstream, crate::configuration::get_upstream,
        crate::configuration::replace_upstream, crate::configuration::delete_upstream,
        crate::configuration::add_node, crate::configuration::replace_node, crate::configuration::delete_node,
        crate::configuration::list_listeners, crate::configuration::get_listener, crate::configuration::put_listener,
        crate::configuration::delete_listener, crate::configuration::list_tls_profiles,
        crate::configuration::get_tls_profile, crate::configuration::put_tls_profile,
        crate::configuration::delete_tls_profile, crate::configuration::draft, crate::configuration::validation,
        crate::configuration::apply,
        crate::language::source, crate::language::replace_source, crate::language::check,
        crate::language::format, crate::language::schema,
        crate::language::ast,
        crate::language::ir, crate::language::plan, crate::language::dry_run,
        crate::language::list_revisions, crate::language::get_revision, crate::language::diff_revision,
        crate::language::restore_revision, crate::language::note_revision,
        crate::gateway_runtime::data_plane, crate::gateway_runtime::reload, crate::gateway_runtime::workers,
        crate::gateway_runtime::shutdown, crate::gateway_runtime::upstream_health, crate::gateway_runtime::drain,
        crate::gateway_runtime::restore
    ),
    tags(
        (name = "configuration", description = "Sites, domains, routes, upstreams, listeners and TLS profiles, edited as a draft and applied to the gateway"),
        (name = "gateway", description = "The running gateway: data plane, workers, shutdown and upstream health")
    ),
    modifiers(&HttpConventions),
    components(schemas(
        SnapshotEnvelope,
        ActivateRequest,
        AbortRequest,
        AbortResponse,
        ValidationResponse,
        PreparedResponse,
        ActivatedResponse,
        GatewayStatusResponse,
        IdempotencyReceiptPendingResponse,
        IdempotencyReceiptResponse,
        ReceiptOutcomeResponse,
        ServiceListingResponse,
        ServiceInstanceResponse,
        ProtocolSupportResponse,
        CapabilityResponse,
        ProblemDetails,
        panel_config_model::SiteSort,
        panel_config_model::SiteStatus,
        panel_config_model::SiteKind
    ))
)]
pub struct ApiDoc;
