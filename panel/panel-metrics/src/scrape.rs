use http::{header, HeaderMap};
use subtle::ConstantTimeEq;

/// Where every process serves its metrics.
pub const PATH: &str = "/metrics";

/// The OpenMetrics text format, which Prometheus negotiates by default.
pub const CONTENT_TYPE: &str = "application/openmetrics-text; version=1.0.0; charset=utf-8";

/// The bearer token a scrape must present, when one is configured.
#[derive(Clone, Default, Eq, PartialEq)]
pub struct ScrapeToken(Option<String>);

impl ScrapeToken {
    /// Scrapes need no token; the listener's address limits who can reach
    /// them.
    pub fn none() -> Self {
        Self(None)
    }

    /// Scrapes must present `token` as `Authorization: Bearer <token>`. An
    /// empty token is no token.
    pub fn bearer(token: impl Into<String>) -> Self {
        let token = token.into();
        Self((!token.is_empty()).then_some(token))
    }

    pub fn is_required(&self) -> bool {
        self.0.is_some()
    }

    /// Whether a request with `headers` may read the metrics. The token is
    /// compared in constant time.
    pub fn admits(&self, headers: &HeaderMap) -> bool {
        let Some(expected) = &self.0 else {
            return true;
        };
        let presented = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| {
                let (scheme, token) = value.split_once(' ')?;
                scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
            });
        presented.is_some_and(|token| bool::from(token.as_bytes().ct_eq(expected.as_bytes())))
    }
}

impl std::fmt::Debug for ScrapeToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ScrapeToken")
            .field(&self.0.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;

    fn headers(authorization: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(authorization).unwrap(),
        );
        headers
    }

    #[test]
    fn a_configured_token_must_be_presented_as_a_bearer_token() {
        let token = ScrapeToken::bearer("s3cret");
        assert!(token.admits(&headers("Bearer s3cret")));
        assert!(token.admits(&headers("bearer s3cret")));
        assert!(!token.admits(&headers("Bearer s3cre")));
        assert!(!token.admits(&headers("Basic s3cret")));
        assert!(!token.admits(&HeaderMap::new()));
        assert_eq!(format!("{token:?}"), "ScrapeToken(Some(\"<redacted>\"))");
    }

    #[test]
    fn without_a_token_every_scrape_is_admitted() {
        assert!(ScrapeToken::none().admits(&HeaderMap::new()));
        assert!(!ScrapeToken::bearer("").is_required());
    }
}
