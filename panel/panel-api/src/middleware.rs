//! Request identity and error rendering are applied once at the router boundary.

use crate::{
    error::render_problem,
    request_context::{trace_context, CORRELATION_ID_HEADER, REQUEST_ID_HEADER},
};
use axum::{
    extract::Request,
    http::HeaderName,
    middleware::{from_fn, Next},
    response::Response,
    Router,
};
use panel_application::RequestId;
use tower::ServiceBuilder;
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    trace::TraceLayer,
};

pub(crate) fn apply(router: Router) -> Router {
    let name = HeaderName::from_static(REQUEST_ID_HEADER);
    router.layer(
        ServiceBuilder::new()
            .map_request(normalize_request_id)
            .layer(SetRequestIdLayer::new(name.clone(), MakeRequestUuid))
            .layer(TraceLayer::new_for_http().make_span_with(request_span))
            .layer(PropagateRequestIdLayer::new(name))
            .layer(from_fn(render_errors)),
    )
}

fn normalize_request_id(mut request: Request) -> Request {
    let mut values = request.headers().get_all(REQUEST_ID_HEADER).iter();
    let valid = values
        .next()
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| RequestId::new(value).is_ok())
        && values.next().is_none();
    if !valid {
        // Removing malformed, oversized or ambiguous identifiers lets the
        // standard Tower UUID layer create one safe identity before tracing.
        request.headers_mut().remove(REQUEST_ID_HEADER);
        request
            .extensions_mut()
            .remove::<tower_http::request_id::RequestId>();
    }
    request
}

/// The request span carries the identity shared with downstream services. It
/// records the path only, because query strings may carry credentials.
fn request_span(request: &Request) -> tracing::Span {
    let header = |name| {
        request
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .filter(|value| RequestId::new(*value).is_ok())
            .unwrap_or_default()
    };
    let request_id = header(REQUEST_ID_HEADER);
    let correlation_id = Some(header(CORRELATION_ID_HEADER))
        .filter(|value| !value.is_empty())
        .unwrap_or(request_id);
    let trace = trace_context(request.headers());
    tracing::info_span!(
        "request",
        method = %request.method(),
        path = request.uri().path(),
        request_id,
        correlation_id,
        trace_id = trace.as_ref().map(|trace| trace.trace_id()).unwrap_or_default(),
    )
}

async fn render_errors(request: Request, next: Next) -> Response {
    let id = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    render_problem(next.run(request).await, id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };
    use tracing::{
        field::{Field, Visit},
        span, Event, Metadata, Subscriber,
    };

    #[derive(Clone, Default)]
    struct SpanFields(Arc<Mutex<BTreeMap<String, String>>>);

    impl Visit for SpanFields {
        fn record_str(&mut self, field: &Field, value: &str) {
            self.0
                .lock()
                .unwrap()
                .insert(field.name().into(), value.into());
        }

        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.0
                .lock()
                .unwrap()
                .insert(field.name().into(), format!("{value:?}"));
        }
    }

    impl Subscriber for SpanFields {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, span: &span::Attributes<'_>) -> span::Id {
            span.record(&mut self.clone());
            span::Id::from_u64(1)
        }
        fn record(&self, _: &span::Id, _: &span::Record<'_>) {}
        fn record_follows_from(&self, _: &span::Id, _: &span::Id) {}
        fn event(&self, _: &Event<'_>) {}
        fn enter(&self, _: &span::Id) {}
        fn exit(&self, _: &span::Id) {}
    }

    fn span_fields(request: &Request) -> BTreeMap<String, String> {
        let fields = SpanFields::default();
        tracing::subscriber::with_default(fields.clone(), || drop(request_span(request)));
        let recorded = fields.0.lock().unwrap().clone();
        recorded
    }

    #[test]
    fn request_spans_carry_identity_but_not_query_strings() {
        let request = Request::builder()
            .uri("/api/v1/gateway/status?token=secret")
            .header(REQUEST_ID_HEADER, "req-1")
            .header(CORRELATION_ID_HEADER, "flow-1")
            .header(
                "traceparent",
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            )
            .body(axum::body::Body::empty())
            .unwrap();
        let fields = span_fields(&request);
        assert_eq!(fields["path"], "/api/v1/gateway/status");
        assert_eq!(fields["request_id"], "req-1");
        assert_eq!(fields["correlation_id"], "flow-1");
        assert_eq!(fields["trace_id"], "4bf92f3577b34da6a3ce929d0e0e4736");
        assert!(!fields.values().any(|value| value.contains("secret")));

        let request = Request::builder()
            .uri("/api/v1/gateway/status")
            .header(REQUEST_ID_HEADER, "req-2")
            .body(axum::body::Body::empty())
            .unwrap();
        let fields = span_fields(&request);
        assert_eq!(fields["correlation_id"], "req-2");
        assert_eq!(fields["trace_id"], "");
    }
}
