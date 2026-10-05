use crate::{
    access, acme,
    admission::{admit, Admission},
    alerts, approvals, audit, certificates, compose, configuration as config, container_sites,
    containers, engine_resources, gateway_runtime as runtime, grants, host, host_agent, identity,
    images, language, logs, middleware, routes, sign_in, site_files, tls_checks, traffic, workload,
    ApiConfig, ApiState,
};
use axum::{
    extract::DefaultBodyLimit,
    handler::Handler,
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
            "/api/v1/gateway/file-checks",
            get(runtime::file_checks::<U>),
        )
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
        .route(
            "/api/v1/security-policies",
            get(config::list_security_policies::<U>),
        )
        .route(
            "/api/v1/security-policies/{id}",
            get(config::get_security_policy::<U>)
                .put(config::put_security_policy::<U>)
                .delete(config::delete_security_policy::<U>),
        )
        .route("/api/v1/config/draft", get(config::draft::<U>))
        .route("/api/v1/config/validation", get(config::validation::<U>))
        .route("/api/v1/config/apply", post(config::apply::<U>))
        .route(
            "/api/v1/approval-policies",
            get(approvals::list_approval_policies::<U>),
        )
        .route(
            "/api/v1/approval-policies/{id}",
            get(approvals::get_approval_policy::<U>)
                .put(approvals::put_approval_policy::<U>)
                .delete(approvals::delete_approval_policy::<U>),
        )
        .route(
            "/api/v1/approvals",
            get(approvals::list_approval_requests::<U>),
        )
        .route(
            "/api/v1/approvals/{id}",
            get(approvals::get_approval_request::<U>),
        )
        .route(
            "/api/v1/approvals/{id}/approve",
            post(approvals::approve_request::<U>),
        )
        .route(
            "/api/v1/approvals/{id}/reject",
            post(approvals::reject_request::<U>),
        )
        .route(
            "/api/v1/approvals/{id}/revoke",
            post(approvals::revoke_approval::<U>),
        )
        .route(
            "/api/v1/approvals/{id}/withdraw",
            post(approvals::withdraw_request::<U>),
        )
        .route(
            "/api/v1/config/source",
            get(language::source::<U>).put(language::replace_source::<U>),
        )
        .route(
            "/api/v1/config/bundle",
            get(language::bundle::<U>).put(language::import_bundle::<U>),
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
        .route("/api/v1/traffic", get(traffic::traffic_summary::<U>))
        .route("/api/v1/traffic/series", get(traffic::traffic_series::<U>))
        .route("/api/v1/logs", get(logs::search_logs::<U>))
        .route("/api/v1/logs/download", get(logs::download_logs::<U>))
        .route(
            "/api/v1/logs/tail",
            get(logs::tail_logs::<U>).connect(logs::tail_logs::<U>),
        )
        .route(
            "/api/v1/logs/deletions",
            get(logs::list_log_deletions::<U>).post(logs::delete_logs::<U>),
        )
        .route("/api/v1/host", get(host::host_summary::<U>))
        .route("/api/v1/host/agent", get(host_agent::host_agent::<U>))
        .route(
            "/api/v1/host/directories",
            get(host_agent::host_directories::<U>),
        )
        .route(
            "/api/v1/host/listeners",
            get(host_agent::host_listeners::<U>),
        )
        .route(
            "/api/v1/host/gateway-service",
            get(host_agent::gateway_service::<U>),
        )
        .route(
            "/api/v1/container-engines",
            get(containers::list_engines::<U>),
        )
        .route(
            "/api/v1/container-engines/{engine}/enable",
            post(containers::enable_engine::<U>),
        )
        .route(
            "/api/v1/container-engines/{engine}/disable",
            post(containers::disable_engine::<U>),
        )
        .route(
            "/api/v1/container-engines/{engine}/containers",
            get(containers::list_containers::<U>),
        )
        .route(
            "/api/v1/container-engines/{engine}/containers/{container}",
            get(containers::inspect_container::<U>).delete(containers::remove_container::<U>),
        )
        .route(
            "/api/v1/container-engines/{engine}/containers/{container}/{action}",
            post(containers::act_on_container::<U>),
        )
        .route(
            "/api/v1/container-engines/{engine}/containers/{container}/logs",
            get(containers::container_logs::<U>),
        )
        .route(
            "/api/v1/container-engines/{engine}/containers/{container}/stats",
            get(containers::container_stats::<U>),
        )
        .route(
            "/api/v1/container-engines/{engine}/stats",
            get(containers::list_container_stats::<U>),
        )
        .route(
            "/api/v1/container-engines/{engine}/images",
            get(images::list_images::<U>),
        )
        .route(
            "/api/v1/container-engines/{engine}/networks",
            get(engine_resources::list_networks::<U>),
        )
        .route(
            "/api/v1/container-engines/{engine}/volumes",
            get(engine_resources::list_volumes::<U>),
        )
        .route(
            "/api/v1/container-engines/{engine}/disk-usage",
            get(engine_resources::disk_usage::<U>),
        )
        .route(
            "/api/v1/container-engines/{engine}/prune-preview",
            get(engine_resources::prune_preview::<U>),
        )
        .route(
            "/api/v1/container-engines/{engine}/prune",
            post(engine_resources::prune::<U>),
        )
        .route(
            "/api/v1/container-engines/{engine}/image-pulls",
            post(images::pull_image::<U>),
        )
        .route(
            "/api/v1/container-engines/{engine}/site-links",
            get(container_sites::site_links::<U>),
        )
        .route(
            "/api/v1/site-files",
            get(site_files::list_directory::<U>).delete(site_files::remove_entry::<U>),
        )
        .route(
            "/api/v1/site-files/content",
            get(site_files::read_file::<U>).put(
                site_files::write_file::<U>
                    .layer(DefaultBodyLimit::max(site_files::MOST_WRITE_BYTES)),
            ),
        )
        .route(
            "/api/v1/site-files/directories",
            post(site_files::create_directory::<U>),
        )
        .route(
            "/api/v1/container-engines/{engine}/containers/{container}/sites",
            post(container_sites::create_container_site::<U>),
        )
        .route(
            "/api/v1/container-engines/{engine}/compose-projects",
            get(compose::list_projects::<U>),
        )
        .route(
            "/api/v1/container-engines/{engine}/compose-projects/{project}/logs",
            get(compose::project_logs::<U>),
        )
        .route(
            "/api/v1/container-engines/{engine}/compose-projects/{project}/files",
            get(compose::project_files::<U>),
        )
        .route(
            "/api/v1/container-engines/{engine}/compose-projects/{project}/{action}",
            post(compose::act_on_project::<U>),
        )
        .route(
            "/api/v1/container-engines/{engine}/images/{image}",
            get(images::inspect_image::<U>).delete(images::remove_image::<U>),
        )
        .route(
            "/api/v1/container-engines/{engine}/containers/{container}/logs/tail",
            get(containers::tail_container_logs::<U>).connect(containers::tail_container_logs::<U>),
        )
        .route(
            "/api/v1/host/gateway-service/{action}",
            post(host_agent::change_gateway_service::<U>),
        )
        .route("/api/v1/alert-rules", get(alerts::list_alert_rules::<U>))
        .route(
            "/api/v1/alert-rules/{id}",
            put(alerts::put_alert_rule::<U>).delete(alerts::delete_alert_rule::<U>),
        )
        .route(
            "/api/v1/alert-channels",
            get(alerts::list_alert_channels::<U>).post(alerts::create_alert_channel::<U>),
        )
        .route(
            "/api/v1/alert-channels/{id}",
            delete(alerts::delete_alert_channel::<U>),
        )
        .route(
            "/api/v1/alert-channels/{id}/rotate",
            post(alerts::rotate_alert_channel::<U>),
        )
        .route(
            "/api/v1/alert-channels/{id}/test",
            post(alerts::test_alert_channel::<U>),
        )
        .route(
            "/api/v1/alert-notifications",
            get(alerts::list_alert_notifications::<U>),
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
        .route(
            "/api/v1/acme-accounts",
            get(acme::list_acme_accounts::<U>).post(acme::create_acme_account::<U>),
        )
        .route(
            "/api/v1/acme-accounts/{id}",
            get(acme::get_acme_account::<U>).delete(acme::delete_acme_account::<U>),
        )
        .route(
            "/api/v1/acme-certificates",
            get(acme::list_automatic_certificates::<U>)
                .post(acme::create_automatic_certificate::<U>),
        )
        .route(
            "/api/v1/acme-certificates/{id}",
            get(acme::get_automatic_certificate::<U>)
                .delete(acme::delete_automatic_certificate::<U>),
        )
        .route(
            "/api/v1/acme-certificates/{id}/renewals",
            post(acme::renew_automatic_certificate::<U>),
        )
        .route(
            "/api/v1/dns-providers",
            get(acme::list_dns_providers::<U>).post(acme::create_dns_provider::<U>),
        )
        .route(
            "/api/v1/dns-providers/{id}",
            get(acme::get_dns_provider::<U>)
                .put(acme::update_dns_provider::<U>)
                .delete(acme::delete_dns_provider::<U>),
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
            get(identity::account_tokens::<U>).post(identity::issue_token::<U>),
        )
        .route(
            "/api/v1/accounts/{id}/grants",
            get(grants::list_grants::<U>).post(grants::create_grant::<U>),
        )
        .route(
            "/api/v1/accounts/{id}/grants/{grant}",
            delete(grants::delete_grant::<U>),
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
        .route(
            "/api/v1/identity-providers",
            get(sign_in::list_identity_providers::<U>),
        )
        .route(
            "/api/v1/identity-providers/{id}",
            get(sign_in::get_identity_provider::<U>)
                .put(sign_in::put_identity_provider::<U>)
                .delete(sign_in::delete_identity_provider::<U>),
        )
        .route(
            "/api/v1/sign-in-policy",
            get(sign_in::get_sign_in_policy::<U>).put(sign_in::put_sign_in_policy::<U>),
        )
        .route(
            "/api/v1/workload-identities",
            get(workload::list_workload_identities::<U>),
        )
        .route(
            "/api/v1/workload-identities/{id}",
            get(workload::get_workload_identity::<U>)
                .put(workload::put_workload_identity::<U>)
                .delete(workload::delete_workload_identity::<U>),
        )
        .route(
            "/api/v1/auth/workload",
            post(workload::exchange_workload_token::<U>),
        )
        .route("/api/v1/auth/providers", get(sign_in::sign_in_options::<U>))
        .route(
            "/api/v1/auth/oidc/{id}/start",
            get(sign_in::start_sign_in::<U>),
        )
        .route(
            "/api/v1/auth/oidc/{id}/callback",
            get(sign_in::finish_sign_in::<U>),
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
