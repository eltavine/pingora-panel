use crate::{
    access,
    admission::{admit, Admission},
    audit, certificates, configuration as config, gateway_runtime as runtime, identity, language,
    middleware, routes, tls_checks, ApiConfig, ApiState,
};
use axum::{
    extract::DefaultBodyLimit,
    middleware::{from_fn, from_fn_with_state},
    routing::{delete, get, post, put},
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
        .route("/api/v1/config/explain", post(language::explain::<U>))
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
        .route(
            "/api/v1/certificates",
            get(certificates::list_certificates::<U>).post(certificates::create_certificate::<U>),
        )
        .route(
            "/api/v1/certificates/{id}",
            get(certificates::get_certificate::<U>)
                .put(certificates::replace_certificate::<U>)
                .delete(certificates::delete_certificate::<U>),
        )
        .route(
            "/api/v1/certificates/{id}/coverage",
            get(certificates::certificate_coverage::<U>),
        )
        .route(
            "/api/v1/certificate-inspections",
            post(certificates::inspect_certificate),
        )
        .route("/api/v1/tls-checks", post(tls_checks::check_tls::<U>))
        .route("/api/v1/openapi.json", get(routes::openapi))
        .route(
            "/api/v1/setup",
            get(identity::setup_status::<U>).post(identity::setup::<U>),
        )
        .route(
            "/api/v1/session",
            get(identity::session::<U>)
                .post(identity::login::<U>)
                .delete(identity::logout::<U>),
        )
        .route("/api/v1/permissions", get(identity::permissions))
        .route(
            "/api/v1/account/password",
            put(identity::change_password::<U>),
        )
        .route(
            "/api/v1/account/sessions",
            get(identity::own_sessions::<U>).delete(identity::end_other_sessions::<U>),
        )
        .route(
            "/api/v1/account/sessions/{id}",
            delete(identity::end_own_session::<U>),
        )
        .route(
            "/api/v1/account/tokens",
            get(identity::own_tokens::<U>).post(identity::create_token::<U>),
        )
        .route(
            "/api/v1/account/tokens/{id}",
            delete(identity::revoke_own_token::<U>),
        )
        .route(
            "/api/v1/account/tokens/{id}/rotate",
            post(identity::rotate_token::<U>),
        )
        .route(
            "/api/v1/accounts",
            get(identity::list_accounts::<U>).post(identity::create_account::<U>),
        )
        .route(
            "/api/v1/accounts/{id}",
            get(identity::get_account::<U>).patch(identity::update_account::<U>),
        )
        .route(
            "/api/v1/accounts/{id}/password",
            put(identity::reset_password::<U>),
        )
        .route(
            "/api/v1/accounts/{id}/sessions",
            get(identity::account_sessions::<U>).delete(identity::end_account_sessions::<U>),
        )
        .route(
            "/api/v1/accounts/{id}/sessions/{session}",
            delete(identity::end_account_session::<U>),
        )
        .route(
            "/api/v1/accounts/{id}/tokens",
            get(identity::account_tokens::<U>),
        )
        .route(
            "/api/v1/accounts/{id}/tokens/{token}",
            delete(identity::revoke_account_token::<U>),
        )
        .route(
            "/api/v1/roles",
            get(identity::list_roles::<U>).post(identity::create_role::<U>),
        )
        .route(
            "/api/v1/roles/{id}",
            put(identity::replace_role::<U>).delete(identity::delete_role::<U>),
        )
        .route_layer(from_fn_with_state(state.clone(), access::guard::<U>))
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
