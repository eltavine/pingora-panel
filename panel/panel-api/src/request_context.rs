use crate::error::ApiError;
use axum::http::HeaderMap;
use panel_application::{CommandContext, IdempotencyKey, RequestDeadline, RequestId};
use panel_errors::PanelError;

pub(crate) const REQUEST_ID_HEADER: &str = "x-request-id";

/// Metadata accepted by mutating endpoints. The same type defines the
/// OpenAPI headers and carries their parsed values to application validation.
#[derive(utoipa::IntoParams)]
#[into_params(parameter_in = Header)]
pub(crate) struct MutationHeaders {
    /// Caller-supplied actor metadata; this header does not authenticate identity.
    #[param(rename = "x-actor", min_length = 1, max_length = 256)]
    actor: String,
    /// Absolute request deadline in RFC 3339 format.
    #[param(rename = "x-deadline")]
    deadline: String,
    /// Idempotency identity, containing 1..=256 visible ASCII bytes.
    #[param(rename = "Idempotency-Key", min_length = 1, max_length = 256)]
    idempotency_key: String,
    /// Optional correlation identity; defaults to the request identifier.
    #[param(rename = "x-correlation-id", min_length = 1, max_length = 256)]
    correlation_id: Option<String>,
}

impl MutationHeaders {
    fn parse(headers: &HeaderMap) -> Result<Self, ApiError> {
        Ok(Self {
            actor: required_header(headers, "x-actor")?.into(),
            deadline: required_header(headers, "x-deadline")?.into(),
            idempotency_key: required_header(headers, "idempotency-key")?.into(),
            correlation_id: optional_header(headers, "x-correlation-id")?.map(str::to_owned),
        })
    }
}

pub(crate) fn command_context(headers: &HeaderMap) -> Result<CommandContext, ApiError> {
    let request_id = RequestId::new(required_header(headers, REQUEST_ID_HEADER)?)?;
    let metadata = MutationHeaders::parse(headers)?;
    let correlation_id = RequestId::new(
        metadata
            .correlation_id
            .unwrap_or_else(|| request_id.as_str().into()),
    )?;
    let deadline = RequestDeadline::new(metadata.deadline)?;
    let idempotency_key = IdempotencyKey::new(metadata.idempotency_key)?;
    CommandContext::new(
        request_id,
        correlation_id,
        metadata.actor,
        deadline,
        idempotency_key,
    )
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
