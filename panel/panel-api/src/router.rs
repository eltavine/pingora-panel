use crate::{
    admission::{admit, Admission},
    audit, configuration as config, gateway_runtime as runtime, language, middleware, routes,
    ApiConfig, ApiState,
};
use axum::{
    extract::DefaultBodyLimit,
    middleware::from_fn,
    routing::{get, post, put},
    Router,
};
use panel_application::GatewayUseCases;

pub fn router<U: GatewayUseCases + 'static>(state: ApiState<U>) -> Router {
    router_with_config(state, ApiConfig::default())
}

/// Builds the HTTP adapter with explicit resource policy.
pub fn router_with_config<U: GatewayUseCases + 'static>(
    state: ApiState<U>,
    config: ApiConfig,
) -> Router {
    let admission = state.health.clone().map(|health| Admission {
        health,
        retry_after: config.unavailable_retry_after(),
    });
    let router = Router::new()
        .route("/api/v1/gateway/validate", post(routes::validate::<U>))
        .route("/api/v1/gateway/prepare", post(routes::prepare::<U>))
        .route("/api/v1/gateway/activate", post(routes::activate::<U>))
        .route("/api/v1/gateway/abort", post(routes::abort::<U>))
        .route("/api/v1/gateway/status", get(routes::status::<U>))
        .route("/api/v1/gateway/data-plane", get(runtime::data_plane::<U>))
        .route("/api/v1/gateway/reload", post(runtime::reload::<U>))
        .route("/api/v1/gateway/workers", put(runtime::workers::<U>))
        .route("/api/v1/gateway/shutdown", post(runtime::shutdown::<U>))
        .route(
            "/api/v1/upstreams/health",
            get(runtime::upstream_health::<U>),
        )
        .route(
            "/api/v1/upstreams/{id}/nodes/{node}/drain",
            put(runtime::drain::<U>).delete(runtime::restore::<U>),
        )
        .route("/api/v1/gateway/receipts/{key}", get(routes::receipt::<U>))
        .route("/api/v1/platform/services", get(routes::services::<U>))
        .route(
            "/api/v1/sites",
            get(config::list_sites::<U>).post(config::create_site::<U>),
        )
        .route("/api/v1/sites/summary", get(config::site_summary::<U>))
        .route("/api/v1/sites/export", get(config::export_sites::<U>))
        .route("/api/v1/sites/import", post(config::import_sites::<U>))
        .route("/api/v1/sites/batch", post(config::batch_sites::<U>))
        .route(
            "/api/v1/sites/{id}",
            get(config::get_site::<U>)
                .put(config::replace_site::<U>)
                .delete(config::delete_site::<U>),
        )
        .route("/api/v1/sites/{id}/enable", post(config::enable_site::<U>))
        .route(
            "/api/v1/sites/{id}/disable",
            post(config::disable_site::<U>),
        )
        .route(
            "/api/v1/sites/{id}/favorite",
            post(config::favorite_site::<U>),
        )
        .route(
            "/api/v1/sites/{id}/unfavorite",
            post(config::unfavorite_site::<U>),
        )
        .route(
            "/api/v1/sites/{id}/restore",
            post(config::restore_site::<U>),
        )
        .route("/api/v1/sites/{id}/clone", post(config::clone_site::<U>))
        .route("/api/v1/sites/{id}/domains", post(config::add_domains::<U>))
        .route(
            "/api/v1/sites/{id}/domains/{host}",
            put(config::replace_domain::<U>).delete(config::remove_domain::<U>),
        )
        .route(
            "/api/v1/sites/{id}/routes",
            get(config::list_routes::<U>).post(config::create_route::<U>),
        )
        .route(
            "/api/v1/sites/{id}/routes/order",
            put(config::reorder_routes::<U>),
        )
        .route(
            "/api/v1/routes/{id}",
            get(config::get_route::<U>)
                .put(config::replace_route::<U>)
                .delete(config::delete_route::<U>),
        )
        .route("/api/v1/domains", get(config::list_domains::<U>))
        .route("/api/v1/domains/check", post(config::check_domains::<U>))
        .route(
            "/api/v1/upstreams",
            get(config::list_upstreams::<U>).post(config::create_upstream::<U>),
        )
        .route(
            "/api/v1/upstreams/{id}",
            get(config::get_upstream::<U>)
                .put(config::replace_upstream::<U>)
                .delete(config::delete_upstream::<U>),
        )
        .route("/api/v1/upstreams/{id}/nodes", post(config::add_node::<U>))
        .route(
            "/api/v1/upstreams/{id}/nodes/{node}",
            put(config::replace_node::<U>).delete(config::delete_node::<U>),
        )
        .route("/api/v1/listeners", get(config::list_listeners::<U>))
        .route(
            "/api/v1/listeners/{id}",
            get(config::get_listener::<U>)
                .put(config::put_listener::<U>)
                .delete(config::delete_listener::<U>),
        )
        .route("/api/v1/tls-profiles", get(config::list_tls_profiles::<U>))
        .route(
            "/api/v1/tls-profiles/{id}",
            get(config::get_tls_profile::<U>)
                .put(config::put_tls_profile::<U>)
                .delete(config::delete_tls_profile::<U>),
        )
        .route("/api/v1/config/draft", get(config::draft::<U>))
        .route("/api/v1/config/validation", get(config::validation::<U>))
        .route("/api/v1/config/apply", post(config::apply::<U>))
        .route(
            "/api/v1/config/source",
            get(language::source::<U>).put(language::replace_source::<U>),
        )
        .route("/api/v1/config/check", post(language::check::<U>))
        .route("/api/v1/config/format", post(language::format::<U>))
        .route("/api/v1/config/schema", get(language::schema::<U>))
        .route("/api/v1/config/ast", post(language::ast::<U>))
        .route("/api/v1/config/ir", get(language::ir::<U>))
        .route(
            "/api/v1/config/import/nginx",
            post(language::import_nginx::<U>),
        )
        .route("/api/v1/config/plan", get(language::plan::<U>))
        .route("/api/v1/config/dry-run", post(language::dry_run::<U>))
        .route("/api/v1/revisions", get(language::list_revisions::<U>))
        .route("/api/v1/revisions/{id}", get(language::get_revision::<U>))
        .route(
            "/api/v1/revisions/{id}/diff",
            get(language::diff_revision::<U>),
        )
        .route(
            "/api/v1/revisions/{id}/restore",
            post(language::restore_revision::<U>),
        )
        .route(
            "/api/v1/revisions/{id}/note",
            put(language::note_revision::<U>),
        )
        .route("/api/v1/audit-events", get(audit::list_audit_events::<U>))
        .route(
            "/api/v1/audit-events/verify",
            get(audit::verify_audit_events::<U>),
        )
        .route(
            "/api/v1/audit-events/{sequence}",
            get(audit::get_audit_event::<U>),
        )
        .route("/api/v1/openapi.json", get(routes::openapi))
        .layer(DefaultBodyLimit::max(config.max_body_bytes()))
        .with_state(state);
    let router = match admission {
        Some(admission) => router.layer(from_fn(move |request, next| {
            admit(admission.clone(), request, next)
        })),
        None => router,
    };
    middleware::apply(router)
}
