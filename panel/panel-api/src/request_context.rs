use crate::error::ApiError;
use axum::http::HeaderMap;
use panel_application::{
    CommandContext, IdempotencyKey, RequestDeadline, RequestId, RequestScope, TraceContext,
};
use panel_errors::PanelError;

pub(crate) const REQUEST_ID_HEADER: &str = "x-request-id";
pub(crate) const CORRELATION_ID_HEADER: &str = "x-correlation-id";
pub(crate) const TRACEPARENT_HEADER: &str = "traceparent";
pub(crate) const TRACESTATE_HEADER: &str = "tracestate";

/// Metadata accepted by query endpoints, defining their OpenAPI headers and
/// carrying the parsed values like `MutationHeaders`.
#[derive(utoipa::IntoParams)]
#[into_params(parameter_in = Header)]
pub(crate) struct QueryHeaders {
    /// Optional correlation identity; defaults to the request identifier.
    #[param(rename = "x-correlation-id", min_length = 1, max_length = 256)]
    correlation_id: Option<String>,
}

impl QueryHeaders {
    fn parse(headers: &HeaderMap) -> Result<Self, ApiError> {
        Ok(Self {
            correlation_id: optional_header(headers, CORRELATION_ID_HEADER)?.map(str::to_owned),
        })
    }
}

/// The caller's W3C Trace Context. Following the receiver rules, an invalid
/// or repeated `traceparent` is ignored rather than rejected, and repeated
/// `tracestate` fields are combined in order.
pub(crate) fn trace_context(headers: &HeaderMap) -> Option<TraceContext> {
    let mut parents = headers.get_all(TRACEPARENT_HEADER).iter();
    let parent = parents.next()?.to_str().ok()?;
    if parents.next().is_some() {
        return None;
    }
    let state = headers
        .get_all(TRACESTATE_HEADER)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .collect::<Vec<_>>()
        .join(",");
    TraceContext::parse(parent, (!state.is_empty()).then_some(state.as_str()))
}

/// The identity of a query: its request ID, the caller's correlation ID and
/// trace context.
pub(crate) fn request_scope(headers: &HeaderMap) -> Result<RequestScope, ApiError> {
    let request_id = RequestId::new(required_header(headers, REQUEST_ID_HEADER)?)?;
    let metadata = QueryHeaders::parse(headers)?;
    let scope = RequestScope::new(request_id).with_trace_context(trace_context(headers));
    Ok(match metadata.correlation_id {
        Some(correlation_id) => scope.with_correlation_id(RequestId::new(correlation_id)?),
        None => scope,
    })
}

/// Metadata accepted by mutating endpoints. The same type defines the
/// OpenAPI headers and carries their parsed values to application validation.
/// The actor is the authenticated caller; an API that does not authenticate
/// callers reads it from `x-actor`.
#[derive(utoipa::IntoParams)]
#[into_params(parameter_in = Header)]
pub(crate) struct MutationHeaders {
    #[param(ignore)]
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
            correlation_id: optional_header(headers, CORRELATION_ID_HEADER)?.map(str::to_owned),
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
    .map(|context| context.with_trace_context(trace_context(headers)))
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
