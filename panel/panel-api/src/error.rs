use crate::{
    error_contract::{status_for, PROBLEM_MEDIA_TYPE},
    DiagnosticDetails, ProblemDetails,
};
use axum::{
    extract::rejection::JsonRejection,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use panel_errors::PanelError;

pub(crate) struct ApiError {
    source: Box<PanelError>,
    status_override: Option<StatusCode>,
}

impl ApiError {
    pub(crate) fn new(source: PanelError) -> Self {
        Self {
            source: Box::new(source),
            status_override: None,
        }
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
            request_id: None,
            field_errors: source
                .diagnostics
                .into_iter()
                .map(DiagnosticDetails::from)
                .collect(),
        };
        // The router middleware supplies request identity and serializes once.
        // Typed response extensions avoid parsing or buffering response bodies.
        (status, Extension(PendingProblem(body))).into_response()
    }
}

#[derive(Clone)]
struct PendingProblem(ProblemDetails);

pub(crate) fn render_problem(mut response: Response, request_id: Option<String>) -> Response {
    if let Some(PendingProblem(mut problem)) = response.extensions_mut().remove::<PendingProblem>()
    {
        problem.request_id = request_id;
        let rendered = Json(problem).into_response();
        let (parts, body) = rendered.into_parts();
        response.headers_mut().extend(parts.headers);
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            PROBLEM_MEDIA_TYPE.parse().expect("static media type"),
        );
        *response.body_mut() = body;
    }
    response
}
