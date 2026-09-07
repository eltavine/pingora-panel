use crate::{DiagnosticDetails, ProblemDetails};
use axum::{
    extract::rejection::JsonRejection,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use panel_errors::{ErrorCode, PanelError};

pub(crate) struct ApiError {
    source: Box<PanelError>,
    request_id: Option<String>,
    status_override: Option<StatusCode>,
}

impl ApiError {
    pub(crate) fn new(source: PanelError) -> Self {
        Self {
            source: Box::new(source),
            request_id: None,
            status_override: None,
        }
    }

    pub(crate) fn with_request_id(mut self, request_id: impl Into<String>) -> Self {
        self.request_id = Some(request_id.into());
        self
    }

    pub(crate) fn from_json(error: JsonRejection) -> Self {
        let status = error.status();
        let mut api_error = Self::new(PanelError::invalid_argument(format!(
            "invalid JSON request: {error}"
        )));
        api_error.status_override = Some(status);
        api_error
    }
}

impl From<PanelError> for ApiError {
    fn from(value: PanelError) -> Self {
        Self::new(value)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let source = *self.source;
        let status = self
            .status_override
            .unwrap_or_else(|| status_for(source.code.as_str()));
        let body = ProblemDetails {
            problem_type: format!("urn:pingora-panel:error:{}", source.code),
            title: status
                .canonical_reason()
                .unwrap_or("Request failed")
                .to_string(),
            status: status.as_u16(),
            detail: source.message,
            code: source.code.to_string(),
            retryable: source.retryable,
            request_id: self.request_id,
            field_errors: source
                .diagnostics
                .into_iter()
                .map(DiagnosticDetails::from)
                .collect(),
        };
        (
            status,
            [(header::CONTENT_TYPE, "application/problem+json")],
            Json(body),
        )
            .into_response()
    }
}

fn status_for(code: &str) -> StatusCode {
    match code {
        ErrorCode::INVALID_ARGUMENT | ErrorCode::VALIDATION_FAILED => StatusCode::BAD_REQUEST,
        ErrorCode::CONFLICT => StatusCode::CONFLICT,
        ErrorCode::PRECONDITION_FAILED => StatusCode::PRECONDITION_FAILED,
        ErrorCode::UNSUPPORTED_CAPABILITY => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCode::NOT_FOUND => StatusCode::NOT_FOUND,
        ErrorCode::RESOURCE_EXHAUSTED => StatusCode::TOO_MANY_REQUESTS,
        ErrorCode::DEADLINE_EXCEEDED => StatusCode::REQUEST_TIMEOUT,
        ErrorCode::UNAUTHENTICATED => StatusCode::UNAUTHORIZED,
        ErrorCode::PERMISSION_DENIED => StatusCode::FORBIDDEN,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}
