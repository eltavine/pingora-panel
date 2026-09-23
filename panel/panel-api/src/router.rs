use crate::{middleware, routes, ApiConfig, ApiState};
use axum::{
    extract::DefaultBodyLimit,
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
    middleware::apply(router)
}
