#![forbid(unsafe_code)]

//! Prometheus metrics in the OpenMetrics text format (ADR 0022).
//!
//! Names and labels follow the OpenTelemetry semantic conventions as the
//! OpenTelemetry Prometheus compatibility rules translate them:
//! `http.server.request.duration`, measured in seconds, is
//! `http_server_request_duration_seconds`, and the `http.request.method`
//! attribute is the `http_request_method` label.

mod scrape;
mod semconv;

pub use scrape::{scrape, ScrapeToken, CONTENT_TYPE, PATH};
pub use semconv::{
    method, protocol_version, ActiveRequest, ClientRequest, ErrorType, HttpClientMetrics,
    HttpServerMetrics, RequestLabels, RoutedRequest, ServerRequest, DURATION_BUCKETS, SIZE_BUCKETS,
};

use prometheus_client::registry::Registry;

/// The metrics of one process, registered once at startup and then read by
/// every scrape.
#[derive(Debug, Default)]
pub struct Metrics {
    registry: Registry,
}

impl Metrics {
    pub fn new() -> Self {
        Self::default()
    }

    /// The registry, to add metrics of the process's own concepts.
    pub fn registry(&mut self) -> &mut Registry {
        &mut self.registry
    }

    /// Every metric in the OpenMetrics text format.
    pub fn encode(&self) -> String {
        let mut text = String::new();
        // Writing to a `String` cannot fail.
        let _ = prometheus_client::encoding::text::encode(&mut text, &self.registry);
        text
    }
}
