use axum::{
    body::Body,
    extract::Request,
    http::{header, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Router,
};
use panel_errors::{PanelError, Result};
use std::{collections::BTreeMap, path::Path};
use tower::ServiceExt;
use tower_http::{
    services::{ServeDir, ServeFile},
    set_header::SetResponseHeaderLayer,
};

/// Browser hardening shared with the console's own test server: same-origin
/// scripts, styles, fonts and data only, and no framing.
const SECURITY_HEADERS: &str = include_str!("../../web/security-headers.json");

fn security_headers() -> Result<Vec<(HeaderName, HeaderValue)>> {
    let headers: BTreeMap<String, String> = serde_json::from_str(SECURITY_HEADERS)
        .map_err(|error| PanelError::internal(format!("invalid security headers: {error}")))?;
    headers
        .into_iter()
        .map(|(name, value)| {
            Ok((
                HeaderName::try_from(name.as_str()).map_err(|error| {
                    PanelError::internal(format!("invalid security header name: {error}"))
                })?,
                HeaderValue::try_from(value.as_str()).map_err(|error| {
                    PanelError::internal(format!("invalid security header value: {error}"))
                })?,
            ))
        })
        .collect()
}

/// Serves the web console from `root` for every path the API does not
/// route, falling back to `index.html` for client-side routes. Unknown API
/// paths stay JSON-only. Responses carry browser hardening headers.
pub(crate) fn with_console(api: Router, root: &Path) -> Result<Router> {
    let router = if root.join("index.html").is_file() {
        let files = ServeDir::new(root).fallback(ServeFile::new(root.join("index.html")));
        api.fallback(move |request: Request| {
            let files = files.clone();
            async move {
                if request.uri().path().starts_with("/api/") {
                    return api_not_found();
                }
                match files.oneshot(request).await {
                    Ok(response) => response.into_response(),
                    Err(error) => match error {},
                }
            }
        })
    } else {
        tracing::info!(root = %root.display(), "web console assets not found; serving the API only");
        api.fallback(|| async { api_not_found() })
    };
    Ok(security_headers()?
        .into_iter()
        .fold(router, |router, (name, value)| {
            router.layer(SetResponseHeaderLayer::if_not_present(name, value))
        }))
}

fn api_not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        [(header::CONTENT_TYPE, "application/problem+json")],
        Body::from(
            r#"{"type":"about:blank","title":"Not Found","status":404,"detail":"No API resource exists at this path."}"#,
        ),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shared_security_headers_are_valid_and_strict() {
        let headers = security_headers().unwrap();
        let policy = headers
            .iter()
            .find(|(name, _)| name == header::CONTENT_SECURITY_POLICY)
            .map(|(_, value)| value.to_str().unwrap())
            .unwrap();
        assert!(!policy.contains("unsafe-inline") && !policy.contains("unsafe-eval"));
        assert!(policy.contains("frame-ancestors 'none'"));
    }
}
