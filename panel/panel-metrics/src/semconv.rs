//! HTTP server and client metrics of the OpenTelemetry semantic conventions.

use prometheus_client::{
    encoding::{EncodeLabelSet, EncodeLabelValue, LabelValueEncoder},
    metrics::{family::Family, gauge::Gauge, histogram::Histogram},
    registry::{Registry, Unit},
};
use std::{fmt::Write, sync::Arc, time::Duration};

/// The bucket boundaries, in seconds, the conventions advise for request
/// durations.
pub const DURATION_BUCKETS: [f64; 14] = [
    0.005, 0.01, 0.025, 0.05, 0.075, 0.1, 0.25, 0.5, 0.75, 1.0, 2.5, 5.0, 7.5, 10.0,
];

/// Body size boundaries in bytes: powers of four from 64 B to 64 MiB.
pub const SIZE_BUCKETS: [f64; 11] = [
    64.0,
    256.0,
    1_024.0,
    4_096.0,
    16_384.0,
    65_536.0,
    262_144.0,
    1_048_576.0,
    4_194_304.0,
    16_777_216.0,
    67_108_864.0,
];

/// The `http.request.method` value of `method`: one of the methods the
/// conventions know (RFC 9110, PATCH and QUERY), or `_OTHER`, so a client
/// cannot add label values.
pub fn method(method: &http::Method) -> &'static str {
    match method.as_str() {
        "GET" => "GET",
        "HEAD" => "HEAD",
        "POST" => "POST",
        "PUT" => "PUT",
        "DELETE" => "DELETE",
        "CONNECT" => "CONNECT",
        "OPTIONS" => "OPTIONS",
        "TRACE" => "TRACE",
        "PATCH" => "PATCH",
        "QUERY" => "QUERY",
        _ => "_OTHER",
    }
}

/// The `network.protocol.version` of an HTTP request.
pub fn protocol_version(version: http::Version) -> Option<&'static str> {
    match version {
        http::Version::HTTP_09 => Some("0.9"),
        http::Version::HTTP_10 => Some("1.0"),
        http::Version::HTTP_11 => Some("1.1"),
        http::Version::HTTP_2 => Some("2"),
        http::Version::HTTP_3 => Some("3"),
        _ => None,
    }
}

/// The `error.type` of a request that failed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[non_exhaustive]
pub enum ErrorType {
    /// The status code of a response that reports an error.
    Status(u16),
    /// A low-cardinality identifier, such as `connect_timeout`.
    Named(&'static str),
}

impl EncodeLabelValue for ErrorType {
    fn encode(&self, encoder: &mut LabelValueEncoder) -> Result<(), std::fmt::Error> {
        match self {
            Self::Status(status) => write!(encoder, "{status}"),
            Self::Named(name) => encoder.write_str(name),
        }
    }
}

/// The labels of a request the process served.
#[derive(Clone, Debug, Eq, Hash, PartialEq, EncodeLabelSet)]
pub struct ServerRequest {
    pub http_request_method: &'static str,
    pub url_scheme: &'static str,
    pub http_response_status_code: Option<u16>,
    /// `1.0`, `1.1` or `2`.
    pub network_protocol_version: Option<&'static str>,
    pub error_type: Option<ErrorType>,
    /// The configured site that served the request.
    pub site: Option<Arc<str>>,
    /// The configured route that served the request.
    pub route: Option<Arc<str>>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq, EncodeLabelSet)]
struct ActiveLabels {
    http_request_method: &'static str,
    url_scheme: &'static str,
}

type Histograms<L> = Family<L, Histogram, fn() -> Histogram>;

fn durations<L: Clone + Eq + std::hash::Hash>() -> Histograms<L> {
    Family::new_with_constructor(|| Histogram::new(DURATION_BUCKETS))
}

fn sizes<L: Clone + Eq + std::hash::Hash>() -> Histograms<L> {
    Family::new_with_constructor(|| Histogram::new(SIZE_BUCKETS))
}

/// `http.server.request.duration`, `http.server.active_requests` and the
/// request and response body sizes, whose sums are the traffic in and out.
#[derive(Clone, Debug)]
pub struct HttpServerMetrics {
    duration: Histograms<ServerRequest>,
    active: Family<ActiveLabels, Gauge>,
    request_size: Histograms<ServerRequest>,
    response_size: Histograms<ServerRequest>,
}

impl HttpServerMetrics {
    pub fn register(registry: &mut Registry) -> Self {
        let metrics = Self {
            duration: durations(),
            active: Family::default(),
            request_size: sizes(),
            response_size: sizes(),
        };
        registry.register_with_unit(
            "http_server_request_duration",
            "Duration of HTTP server requests",
            Unit::Seconds,
            metrics.duration.clone(),
        );
        registry.register(
            "http_server_active_requests",
            "Number of active HTTP server requests",
            metrics.active.clone(),
        );
        registry.register_with_unit(
            "http_server_request_body_size",
            "Size of HTTP server request bodies",
            Unit::Bytes,
            metrics.request_size.clone(),
        );
        registry.register_with_unit(
            "http_server_response_body_size",
            "Size of HTTP server response bodies",
            Unit::Bytes,
            metrics.response_size.clone(),
        );
        metrics
    }

    /// Counts a request as active until the returned value is dropped.
    pub fn start(&self, method: &'static str, scheme: &'static str) -> ActiveRequest {
        let gauge = self
            .active
            .get_or_create(&ActiveLabels {
                http_request_method: method,
                url_scheme: scheme,
            })
            .clone();
        gauge.inc();
        ActiveRequest(gauge)
    }

    /// Records a request that is done.
    pub fn finish(
        &self,
        request: &ServerRequest,
        duration: Duration,
        request_bytes: u64,
        response_bytes: u64,
    ) {
        self.duration
            .get_or_create(request)
            .observe(duration.as_secs_f64());
        self.request_size
            .get_or_create(request)
            .observe(request_bytes as f64);
        self.response_size
            .get_or_create(request)
            .observe(response_bytes as f64);
    }
}

/// A request counted in `http.server.active_requests`.
#[derive(Debug)]
pub struct ActiveRequest(Gauge);

impl Drop for ActiveRequest {
    fn drop(&mut self) {
        self.0.dec();
    }
}

/// The labels of a request the process sent to an upstream.
#[derive(Clone, Debug, Eq, Hash, PartialEq, EncodeLabelSet)]
pub struct ClientRequest {
    pub http_request_method: &'static str,
    pub server_address: Arc<str>,
    pub server_port: u16,
    pub http_response_status_code: Option<u16>,
    pub error_type: Option<ErrorType>,
    /// The configured upstream the request went to.
    pub upstream: Arc<str>,
}

/// `http.client.request.duration`.
#[derive(Clone, Debug)]
pub struct HttpClientMetrics {
    duration: Histograms<ClientRequest>,
}

impl HttpClientMetrics {
    pub fn register(registry: &mut Registry) -> Self {
        let duration = durations();
        registry.register_with_unit(
            "http_client_request_duration",
            "Duration of HTTP client requests",
            Unit::Seconds,
            duration.clone(),
        );
        Self { duration }
    }

    pub fn finish(&self, request: &ClientRequest, duration: Duration) {
        self.duration
            .get_or_create(request)
            .observe(duration.as_secs_f64());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Metrics;

    #[test]
    fn protocol_versions_are_named_as_negotiated() {
        assert_eq!(protocol_version(http::Version::HTTP_11), Some("1.1"));
        assert_eq!(protocol_version(http::Version::HTTP_2), Some("2"));
    }

    #[test]
    fn unknown_methods_are_other() {
        assert_eq!(method(&http::Method::GET), "GET");
        assert_eq!(
            method(&http::Method::from_bytes(b"QUERY").unwrap()),
            "QUERY"
        );
        assert_eq!(
            method(&http::Method::from_bytes(b"PURGE").unwrap()),
            "_OTHER"
        );
    }

    #[test]
    fn server_requests_are_exposed_with_semantic_convention_names() {
        let mut metrics = Metrics::new();
        let server = HttpServerMetrics::register(metrics.registry());
        let active = server.start("GET", "https");
        let request = ServerRequest {
            http_request_method: "GET",
            url_scheme: "https",
            http_response_status_code: Some(502),
            network_protocol_version: Some("2"),
            error_type: Some(ErrorType::Status(502)),
            site: Some(Arc::from("shop")),
            route: None,
        };
        server.finish(&request, Duration::from_millis(30), 0, 512);
        let text = metrics.encode();
        assert!(text.contains("# TYPE http_server_request_duration_seconds histogram"));
        assert!(text.contains(
            "# HELP http_server_request_duration_seconds Duration of HTTP server requests.\n"
        ));
        assert!(text.contains(
            "http_server_request_duration_seconds_bucket{le=\"0.05\",http_request_method=\"GET\",\
             url_scheme=\"https\",http_response_status_code=\"502\",\
             network_protocol_version=\"2\",error_type=\"502\",site=\"shop\",route=\"\"} 1"
        ));
        assert!(text.contains(
            "http_server_active_requests{http_request_method=\"GET\",url_scheme=\"https\"} 1"
        ));
        assert!(text.contains("http_server_response_body_size_bytes_sum{"));
        assert!(text.ends_with("# EOF\n"));
        drop(active);
        assert!(metrics.encode().contains(
            "http_server_active_requests{http_request_method=\"GET\",url_scheme=\"https\"} 0"
        ));
    }

    #[test]
    fn client_requests_carry_the_upstream_and_its_address() {
        let mut metrics = Metrics::new();
        let client = HttpClientMetrics::register(metrics.registry());
        client.finish(
            &ClientRequest {
                http_request_method: "POST",
                server_address: Arc::from("10.0.0.7"),
                server_port: 8080,
                http_response_status_code: None,
                error_type: Some(ErrorType::Named("connect_timeout")),
                upstream: Arc::from("api"),
            },
            Duration::from_secs(3),
        );
        assert!(metrics.encode().contains(
            "http_client_request_duration_seconds_count{http_request_method=\"POST\",\
             server_address=\"10.0.0.7\",server_port=\"8080\",http_response_status_code=\"\",\
             error_type=\"connect_timeout\",upstream=\"api\"} 1"
        ));
    }
}
