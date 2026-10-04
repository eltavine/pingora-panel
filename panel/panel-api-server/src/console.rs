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

/// Build output named by its content hash: it never changes, and a missing
/// one is an error rather than a page.
const ASSETS: &str = "/assets/";
const IMMUTABLE: HeaderValue = HeaderValue::from_static("public, max-age=31536000, immutable");
const REVALIDATE: HeaderValue = HeaderValue::from_static("no-cache");

/// Serves the web console from `root` for every path the API does not
/// route, falling back to `index.html` for client-side routes. Unknown API
/// paths stay JSON-only. Files go out in the Brotli or gzip encoding the
/// build compressed them in when the client accepts it, hashed assets are
/// cached for good and everything else is revalidated. Responses carry
/// browser hardening headers.
pub(crate) fn with_console(api: Router, root: &Path) -> Result<Router> {
    let router = if root.join("index.html").is_file() {
        let assets = ServeDir::new(root).precompressed_br().precompressed_gzip();
        let pages = assets.clone().fallback(
            ServeFile::new(root.join("index.html"))
                .precompressed_br()
                .precompressed_gzip(),
        );
        api.fallback(move |request: Request| {
            let (assets, pages) = (assets.clone(), pages.clone());
            async move {
                let path = request.uri().path();
                if path.starts_with("/api/") {
                    return api_not_found();
                }
                let hashed = path.starts_with(ASSETS);
                let served = if hashed {
                    assets.oneshot(request).await
                } else {
                    pages.oneshot(request).await
                };
                let mut response = match served {
                    Ok(response) => response.into_response(),
                    Err(error) => match error {},
                };
                let cache = if hashed && response.status().is_success() {
                    IMMUTABLE
                } else {
                    REVALIDATE
                };
                response.headers_mut().insert(header::CACHE_CONTROL, cache);
                response
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

    async fn get(router: &Router, path: &str, encodings: &str) -> Response {
        let request = Request::builder()
            .uri(path)
            .header(header::ACCEPT_ENCODING, encodings)
            .body(Body::empty())
            .unwrap();
        router.clone().oneshot(request).await.unwrap()
    }

    fn console() -> (tempfile::TempDir, Router) {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("assets")).unwrap();
        for (file, content) in [
            ("index.html", "<title>Pingora Panel</title>"),
            ("index.html.br", "brotli page"),
            ("assets/index-4f1c9a0d.js", "export {}"),
            ("assets/index-4f1c9a0d.js.br", "brotli script"),
            ("assets/index-4f1c9a0d.js.gz", "gzip script"),
        ] {
            std::fs::write(root.path().join(file), content).unwrap();
        }
        let router = with_console(Router::new(), root.path()).unwrap();
        (root, router)
    }

    #[tokio::test]
    async fn hashed_assets_go_out_precompressed_and_are_cached_for_good() {
        let (_root, router) = console();

        let brotli = get(&router, "/assets/index-4f1c9a0d.js", "gzip, br").await;
        assert_eq!(brotli.status(), StatusCode::OK);
        assert_eq!(brotli.headers()[header::CONTENT_ENCODING], "br");
        assert_eq!(brotli.headers()[header::VARY], "accept-encoding");
        assert_eq!(
            brotli.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );

        let gzip = get(&router, "/assets/index-4f1c9a0d.js", "gzip").await;
        assert_eq!(gzip.headers()[header::CONTENT_ENCODING], "gzip");

        let identity = get(&router, "/assets/index-4f1c9a0d.js", "identity").await;
        assert!(identity.headers().get(header::CONTENT_ENCODING).is_none());
    }

    #[tokio::test]
    async fn a_missing_asset_is_not_answered_with_the_page() {
        let (_root, router) = console();

        let missing = get(&router, "/assets/index-00000000.js", "br").await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert_eq!(missing.headers()[header::CACHE_CONTROL], "no-cache");
    }

    #[tokio::test]
    async fn pages_are_revalidated_and_client_routes_get_the_page() {
        let (_root, router) = console();

        for path in ["/", "/sites/shop"] {
            let page = get(&router, path, "br").await;
            assert_eq!(page.status(), StatusCode::OK, "{path}");
            assert_eq!(page.headers()[header::CONTENT_ENCODING], "br", "{path}");
            assert_eq!(page.headers()[header::CACHE_CONTROL], "no-cache", "{path}");
            assert!(page.headers().contains_key(header::CONTENT_SECURITY_POLICY));
        }
        let api = get(&router, "/api/v1/unknown", "br").await;
        assert_eq!(api.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            api.headers()[header::CONTENT_TYPE],
            "application/problem+json"
        );
    }

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
