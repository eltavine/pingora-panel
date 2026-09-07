use crate::error::ApiError;
use axum::http::HeaderMap;
use panel_application::{CommandContext, IdempotencyKey, RequestDeadline, RequestId};
use panel_errors::PanelError;

const IDEMPOTENCY_HEADER: &str = "idempotency-key";

pub(crate) fn command_context(headers: &HeaderMap) -> Result<CommandContext, ApiError> {
    let request_id = RequestId::new(required_header(headers, "x-request-id")?)?;
    let correlation_id = RequestId::new(
        optional_header(headers, "x-correlation-id")?.unwrap_or_else(|| request_id.as_str()),
    )?;
    let actor = required_header(headers, "x-actor")?;
    let deadline = RequestDeadline::new(required_header(headers, "x-deadline")?)?;
    let idempotency_key = IdempotencyKey::new(required_header(headers, IDEMPOTENCY_HEADER)?)?;
    CommandContext::new(request_id, correlation_id, actor, deadline, idempotency_key)
        .map_err(Into::into)
}

fn required_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, ApiError> {
    optional_header(headers, name)?.ok_or_else(|| {
        ApiError::new(PanelError::invalid_argument(format!(
            "{name} header is required"
        )))
    })
}

fn optional_header<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, ApiError> {
    headers
        .get(name)
        .map(|value| {
            value.to_str().map_err(|_| {
                ApiError::new(PanelError::invalid_argument(format!(
                    "{name} header must contain visible ASCII"
                )))
            })
        })
        .transpose()
        .map(|value| value.filter(|value| !value.is_empty()))
}
