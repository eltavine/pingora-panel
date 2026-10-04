# 0022: Metrics, logs and traces

Status: accepted.

## Context

Operators need to see what the gateway serves: request rates, status
classes, latency percentiles, upstream latency and errors, connections, TLS
handshakes and which configuration is active. They need access and error
logs they can tail and search, and traces that follow a request from the
public API through the services. The gateway's request path must never wait
for a telemetry backend: a full queue drops and counts telemetry rather than
blocking a request. Labels must stay bounded, so a client cannot create
series with request paths, addresses or headers.

Widely used formats and conventions already exist for each signal:
[OpenTelemetry semantic conventions](https://opentelemetry.io/docs/specs/semconv/http/http-metrics/)
name HTTP metrics and attributes, Prometheus scrapes the
[OpenMetrics](https://prometheus.io/docs/specs/om/open_metrics_spec/) text
format, [OTLP](https://opentelemetry.io/docs/specs/otlp/) carries logs and
traces to a collector, and [W3C Trace Context](https://www.w3.org/TR/trace-context/)
propagates trace identity.

## Decision

**Metrics.** Every process serves `GET /metrics` on its operational
listener in the OpenMetrics text format, using the Prometheus project's
`prometheus-client` crate. Metric and label names follow the OpenTelemetry
semantic conventions, translated by the
[OpenTelemetry Prometheus compatibility rules](https://opentelemetry.io/docs/specs/otel/compatibility/prometheus_and_openmetrics/):
dots become underscores, the unit becomes a suffix and counters end in
`_total`. The gateway records

- `http_server_request_duration_seconds`, a histogram with the
  conventions' bucket boundaries, by `http_request_method`,
  `http_response_status_code`, `url_scheme`, `network_protocol_version`
  and `error_type`, plus the `site` and `route` identifiers;
- `http_server_active_requests`;
- `http_server_request_body_size_bytes` and
  `http_server_response_body_size_bytes`, whose sums are the traffic in and
  out;
- `http_client_request_duration_seconds` for upstream requests, by
  `upstream` and `error_type`;
- `pingora_panel_gateway_domain_requests_total`, the requests a site took
  by one of its domains, by `site` and the `domain` as configured, such as
  `*.shop.example`; requests a listener's default site takes for a host
  no domain names are not counted;
- `pingora_panel_gateway_upstream_connections_total`, the connections used
  to reach each `upstream`, by whether they were `reused` from the pool, so
  the share reused shows how well the pool keeps connections.

A method outside the known set is recorded as `_OTHER`, as the conventions
require. Sites, routes and upstreams are identifiers from the active
configuration, so every label is bounded by configuration, never by
traffic. Metrics of the product's own concepts, such as the active
configuration generation, carry a `pingora_panel_` prefix. The endpoint
names internal state, so it binds a loopback address by default and, when
a scrape token is configured, requires it as a bearer token compared in
constant time.

**Logs.** The gateway writes access and error logs as JSON lines whose keys
are OpenTelemetry attribute names, or as Combined Log Format, to rotated
files that an OpenTelemetry Collector ships to Loki over OTLP. A bounded
writer drops and counts records it cannot queue. Sensitive request headers
are replaced before a record is written.

**Traces.** Services and the gateway continue W3C Trace Context and export
spans over OTLP to a collector named by the standard `OTEL_EXPORTER_OTLP_*`
variables; without one, they export nothing.

**Queries.** The console and the CLI read telemetry only through
`observability-service`, which queries Prometheus and Loki over their HTTP
APIs and holds their credentials.

## Alternatives

- The OpenTelemetry metrics SDK with its Prometheus exporter would hash an
  attribute list on every request and needs the separate `prometheus`
  crate as well; the gateway only needs pull-based metrics.
- The `metrics` facade installs a process-global recorder, which hides
  which component owns a metric and makes tests share state.
- Pushing metrics over OTLP alone would make Prometheus's OTLP receiver a
  hard dependency; a pull endpoint works with any Prometheus-compatible
  scraper.

## Consequences

- Dashboards and alerts written against the semantic conventions work for
  the gateway without translation.
- Adding a label is a contract change: it must be bounded by configuration
  and reviewed like any other change to a public name.
- The gateway's telemetry stays correct when Prometheus, Loki or a collector
  is down; it only loses what it could not queue, and counts it.
