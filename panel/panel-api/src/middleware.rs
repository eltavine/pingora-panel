//! Request identity and error rendering are applied once at the router boundary.

use crate::{error::render_problem, request_context::REQUEST_ID_HEADER};
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
            .layer(TraceLayer::new_for_http())
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

async fn render_errors(request: Request, next: Next) -> Response {
    let id = request
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    render_problem(next.run(request).await, id)
}
