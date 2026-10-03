//! HTTP to providers over rustls with ring, the panel's TLS stack, without
//! following redirects and with a bound on what a provider may send back.

use bytes::Bytes;
use http::{header, HeaderValue, Method, Request, StatusCode, Uri};
use http_body_util::{BodyExt, Full, Limited};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::TokioExecutor,
};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use rustls::ClientConfig;
use std::time::Duration;

/// The largest answer read from a provider.
const MAX_BODY: usize = 1 << 20;

/// Characters left as they are in form values and query parameters.
pub(crate) const UNRESERVED: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

pub(crate) fn encode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| {
            format!(
                "{}={}",
                utf8_percent_encode(key, UNRESERVED),
                utf8_percent_encode(value, UNRESERVED)
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

pub(crate) struct Http {
    client: Client<HttpsConnector<HttpConnector>, Full<Bytes>>,
    timeout: Duration,
}

impl Http {
    pub(crate) fn new(tls: ClientConfig, timeout: Duration) -> Self {
        let connector = HttpsConnectorBuilder::new()
            .with_tls_config(tls)
            .https_or_http()
            .enable_http1()
            .enable_http2()
            .build();
        Self {
            client: Client::builder(TokioExecutor::new()).build(connector),
            timeout,
        }
    }

    pub(crate) async fn get(&self, uri: &str) -> Result<(StatusCode, Bytes), String> {
        let request = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header(header::ACCEPT, "application/json")
            .body(Full::default())
            .map_err(|error| error.to_string())?;
        self.send(request).await
    }

    pub(crate) async fn post_form(
        &self,
        uri: &str,
        form: &[(&str, &str)],
        authorization: Option<HeaderValue>,
    ) -> Result<(StatusCode, Bytes), String> {
        let mut request = Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header(header::ACCEPT, "application/json")
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        if let Some(authorization) = authorization {
            request = request.header(header::AUTHORIZATION, authorization);
        }
        let request = request
            .body(Full::new(Bytes::from(encode(form))))
            .map_err(|error| error.to_string())?;
        self.send(request).await
    }

    async fn send(&self, request: Request<Full<Bytes>>) -> Result<(StatusCode, Bytes), String> {
        let exchange = async {
            let response = self
                .client
                .request(request)
                .await
                .map_err(|error| error.to_string())?;
            let status = response.status();
            let body = Limited::new(response.into_body(), MAX_BODY)
                .collect()
                .await
                .map_err(|error| error.to_string())?
                .to_bytes();
            Ok((status, body))
        };
        tokio::time::timeout(self.timeout, exchange)
            .await
            .unwrap_or_else(|_| {
                Err(format!(
                    "no answer within {} seconds",
                    self.timeout.as_secs()
                ))
            })
    }
}

/// Whether `uri` is an absolute HTTPS URI, or HTTP on loopback.
pub(crate) fn secure(uri: &str) -> bool {
    let Ok(parsed) = uri.parse::<Uri>() else {
        return false;
    };
    let loopback = parsed.host().is_some_and(|host| {
        host == "localhost"
            || host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    });
    match parsed.scheme_str() {
        Some("https") => parsed.host().is_some(),
        Some("http") => loopback,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_must_be_secure_except_on_loopback() {
        assert!(secure("https://id.example/realm"));
        assert!(secure("http://127.0.0.1:8080"));
        assert!(secure("http://[::1]:8080/realm"));
        assert!(secure("http://localhost/realm"));
        assert!(!secure("http://id.example"));
        assert!(!secure("ftp://id.example"));
        assert!(!secure("/relative"));
        assert_eq!(
            encode(&[("scope", "openid profile"), ("a&b", "c=d/é")]),
            "scope=openid%20profile&a%26b=c%3Dd%2F%C3%A9"
        );
    }
}
