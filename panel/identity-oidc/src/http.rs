//! HTTP to providers through `reqwest` on rustls with ring, the panel's TLS
//! stack, honoring proxy settings, without following redirects and with a
//! bound on what a provider may send back.

use bytes::Bytes;
use reqwest::{
    header::{self, HeaderValue},
    redirect, Client, RequestBuilder, StatusCode,
};
use std::{error::Error, time::Duration};
use url::{Host, Url};

/// The largest answer read from a provider.
const MAX_BODY: usize = 1 << 20;

pub(crate) struct Http {
    client: Client,
}

impl Http {
    pub(crate) fn new(timeout: Duration) -> Result<Self, String> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        Client::builder()
            .redirect(redirect::Policy::none())
            .timeout(timeout)
            .build()
            .map(|client| Self { client })
            .map_err(|error| causes(&error))
    }

    pub(crate) async fn get(&self, uri: &str) -> Result<(StatusCode, Bytes), String> {
        self.send(
            self.client
                .get(uri)
                .header(header::ACCEPT, "application/json"),
        )
        .await
    }

    pub(crate) async fn post_form(
        &self,
        uri: &str,
        form: &[(&str, &str)],
        authorization: Option<HeaderValue>,
    ) -> Result<(StatusCode, Bytes), String> {
        let mut request = self
            .client
            .post(uri)
            .header(header::ACCEPT, "application/json")
            .form(form);
        if let Some(authorization) = authorization {
            request = request.header(header::AUTHORIZATION, authorization);
        }
        self.send(request).await
    }

    async fn send(&self, request: RequestBuilder) -> Result<(StatusCode, Bytes), String> {
        let mut response = request.send().await.map_err(|error| causes(&error))?;
        let status = response.status();
        let mut body = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|error| causes(&error))? {
            if body.len() + chunk.len() > MAX_BODY {
                return Err(format!("the answer is larger than {MAX_BODY} bytes"));
            }
            body.extend_from_slice(&chunk);
        }
        Ok((status, body.into()))
    }
}

/// An error with its causes, such as a certificate the system does not
/// trust.
fn causes(error: &dyn Error) -> String {
    let mut message = error.to_string();
    let mut cause = error.source();
    while let Some(source) = cause {
        message.push_str(": ");
        message.push_str(&source.to_string());
        cause = source.source();
    }
    message
}

/// Whether `uri` is an absolute HTTPS URL, or HTTP on loopback.
pub(crate) fn secure(uri: &str) -> bool {
    let Ok(url) = Url::parse(uri) else {
        return false;
    };
    match (url.scheme(), url.host()) {
        ("https", Some(_)) => true,
        ("http", Some(Host::Domain(domain))) => domain == "localhost",
        ("http", Some(Host::Ipv4(address))) => address.is_loopback(),
        ("http", Some(Host::Ipv6(address))) => address.is_loopback(),
        _ => false,
    }
}

/// A form component (RFC 6749 Appendix B), as client credentials are
/// encoded before Basic authentication.
pub(crate) fn form_component(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
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
        assert_eq!(form_component("a b&c=d/é"), "a+b%26c%3Dd%2F%C3%A9");
    }
}
