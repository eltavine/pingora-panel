//! Request admission by service mode (degraded control-plane operation).

use crate::error::ApiError;
use axum::{extract::Request, middleware::Next, response::IntoResponse, response::Response};
use panel_errors::PanelError;
use panel_health::HealthWatch;
use std::time::Duration;

#[derive(Clone)]
pub(crate) struct Admission {
    pub(crate) health: HealthWatch,
    pub(crate) retry_after: Duration,
}

/// Logins and account changes need only the API's own store, so they are
/// served whenever reads are.
fn needs_only_identity(path: &str) -> bool {
    [
        "/api/v1/setup",
        "/api/v1/session",
        "/api/v1/account",
        "/api/v1/accounts",
        "/api/v1/roles",
        "/api/v1/identity-providers",
        "/api/v1/sign-in-policy",
        "/api/v1/workload-identities",
        "/api/v1/auth/workload",
    ]
    .iter()
    .any(|prefix| {
        path.strip_prefix(prefix)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    })
}

/// Safe methods (RFC 9110 section 9.2.1) need a service that serves reads;
/// every other method needs one that accepts writes.
pub(crate) async fn admit(admission: Admission, request: Request, next: Next) -> Response {
    let mode = admission.health.mode();
    let admitted = if request.method().is_safe() || needs_only_identity(request.uri().path()) {
        mode.accepts_reads()
    } else {
        mode.accepts_writes()
    };
    if admitted {
        return next.run(request).await;
    }
    let message = if mode.accepts_reads() {
        "changes are suspended until required dependencies recover"
    } else {
        "the service is unavailable until required dependencies recover"
    };
    ApiError::from(PanelError::unavailable(message))
        .with_retry_after(admission.retry_after)
        .into_response()
}
