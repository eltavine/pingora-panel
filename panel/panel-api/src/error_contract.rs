//! One source for runtime error statuses and their OpenAPI representation.

use axum::http::StatusCode;
use panel_errors::ErrorCode;

pub(crate) const PROBLEM_MEDIA_TYPE: &str = "application/problem+json";
pub(crate) const ERROR_STATUSES: &[(&str, StatusCode)] = &[
    (ErrorCode::INVALID_ARGUMENT, StatusCode::BAD_REQUEST),
    (ErrorCode::VALIDATION_FAILED, StatusCode::BAD_REQUEST),
    (ErrorCode::CONFLICT, StatusCode::CONFLICT),
    (
        ErrorCode::PRECONDITION_FAILED,
        StatusCode::PRECONDITION_FAILED,
    ),
    (
        ErrorCode::UNSUPPORTED_CAPABILITY,
        StatusCode::UNPROCESSABLE_ENTITY,
    ),
    (ErrorCode::NOT_FOUND, StatusCode::NOT_FOUND),
    (ErrorCode::RESOURCE_EXHAUSTED, StatusCode::TOO_MANY_REQUESTS),
    (ErrorCode::DEADLINE_EXCEEDED, StatusCode::REQUEST_TIMEOUT),
    (ErrorCode::UNAUTHENTICATED, StatusCode::UNAUTHORIZED),
    (ErrorCode::PERMISSION_DENIED, StatusCode::FORBIDDEN),
];

pub(crate) fn status_for(code: &str) -> StatusCode {
    ERROR_STATUSES
        .iter()
        .find_map(|(name, status)| (*name == code).then_some(*status))
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
}

pub(crate) fn public_detail(code: &str, status: StatusCode, message: String) -> String {
    if !status.is_server_error() {
        return message;
    }
    match code {
        ErrorCode::COMMIT_OUTCOME_UNKNOWN => {
            "Operation outcome is unknown; check status and any available receipt before retrying."
                .into()
        }
        ErrorCode::STORAGE_UNAVAILABLE => "A required storage service is unavailable.".into(),
        _ => "The request could not be completed.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ambiguous_commit_keeps_actionable_public_guidance() {
        let detail = public_detail(
            ErrorCode::COMMIT_OUTCOME_UNKNOWN,
            StatusCode::INTERNAL_SERVER_ERROR,
            "filesystem path /secret".into(),
        );
        assert!(detail.contains("receipt"));
        assert!(!detail.contains("/secret"));
    }
}
