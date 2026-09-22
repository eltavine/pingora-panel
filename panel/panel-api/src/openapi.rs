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
    paths(crate::routes::validate, crate::routes::prepare, crate::routes::activate, crate::routes::status, crate::routes::receipt, crate::routes::openapi),
    modifiers(&HttpConventions),
    components(schemas(
        SnapshotEnvelope,
        ActivateRequest,
        ValidationResponse,
        PreparedResponse,
        ActivatedResponse,
        GatewayStatusResponse,
        IdempotencyReceiptPendingResponse,
        IdempotencyReceiptResponse,
        ReceiptOutcomeResponse,
        ProblemDetails
    ))
)]
pub struct ApiDoc;
