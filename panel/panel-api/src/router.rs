use crate::{
    admission::{admit, Admission},
    middleware, routes, ApiConfig, ApiState,
};
use axum::{
    extract::DefaultBodyLimit,
    middleware::from_fn,
    routing::{get, post},
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
        .route("/api/v1/gateway/receipts/{key}", get(routes::receipt::<U>))
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
