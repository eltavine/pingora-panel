//! The fixed PromQL each measure reads (ADR 0027), over the metric names of
//! ADR 0022. Scopes are validated identifiers, quoted besides.

use super::model::{Measure, RuleSpec};
use crate::promql::quoted;
use panel_errors::{PanelError, Result};
use prometheus_http_query::{response::Data, Client};

const RANGE: &str = "5m";
const REQUESTS: &str = "http_server_request_duration_seconds_count";
const REQUEST_BUCKETS: &str = "http_server_request_duration_seconds_bucket";
const UPSTREAM_ATTEMPTS: &str = "http_client_request_duration_seconds_count";

fn selector(matchers: &[String]) -> String {
    if matchers.is_empty() {
        String::new()
    } else {
        format!("{{{}}}", matchers.join(","))
    }
}

/// The query that reads `rule`'s measure now.
pub(crate) fn query(rule: &RuleSpec) -> String {
    let mut requests = Vec::new();
    if let Some(site) = &rule.site {
        requests.push(format!("site={}", quoted(site.as_str())));
    }
    if let Some(route) = &rule.route {
        requests.push(format!("route={}", quoted(route.as_str())));
    }
    let mut upstreams = Vec::new();
    if let Some(upstream) = &rule.upstream {
        upstreams.push(format!("upstream={}", quoted(upstream.as_str())));
    }
    let with = |matchers: &[String], extra: &str| {
        let mut matchers = matchers.to_vec();
        matchers.push(extra.to_owned());
        selector(&matchers)
    };
    match rule.measure {
        Measure::ServerErrorRatio => format!(
            "sum(rate({REQUESTS}{}[{RANGE}])) / sum(rate({REQUESTS}{}[{RANGE}]))",
            with(&requests, "http_response_status_code=~\"5..\""),
            selector(&requests),
        ),
        Measure::LatencyP95 => format!(
            "histogram_quantile(0.95, sum by (le) (rate({REQUEST_BUCKETS}{}[{RANGE}])))",
            selector(&requests),
        ),
        Measure::RequestRate => format!(
            "sum(rate({REQUESTS}{}[{RANGE}])) or vector(0)",
            selector(&requests),
        ),
        Measure::UpstreamErrorRatio => format!(
            "sum(rate({UPSTREAM_ATTEMPTS}{}[{RANGE}])) / sum(rate({UPSTREAM_ATTEMPTS}{}[{RANGE}]))",
            with(&upstreams, "error_type!=\"\""),
            selector(&upstreams),
        ),
        Measure::OpenConnections => "sum(pingora_panel_gateway_open_connections)".to_owned(),
    }
}

/// `rule`'s measure now; `None` when there is no data, such as a ratio of
/// no requests.
pub(crate) async fn read(prometheus: &Client, rule: &RuleSpec) -> Result<Option<f64>> {
    let query = query(rule);
    let response =
        prometheus.query(&query).get().await.map_err(|error| {
            PanelError::unavailable(format!("Prometheus did not answer: {error}"))
        })?;
    Ok(match response.data() {
        Data::Vector(samples) => samples.first().map(|sample| sample.sample().value()),
        Data::Scalar(sample) => Some(sample.value()),
        _ => None,
    }
    .filter(|value| value.is_finite()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alerts::model::{Comparison, Severity};
    use panel_domain::{RouteId, SiteId, UpstreamPoolId};
    use std::time::Duration;

    fn rule(measure: Measure) -> RuleSpec {
        RuleSpec {
            name: "rule".into(),
            description: String::new(),
            measure,
            comparison: Comparison::Above,
            threshold: 0.5,
            pending_for: Duration::ZERO,
            site: None,
            route: None,
            upstream: None,
            severity: Severity::Warning,
            enabled: true,
            channels: Vec::new(),
        }
    }

    #[test]
    fn measures_read_fixed_queries_of_their_scope() {
        let mut errors = rule(Measure::ServerErrorRatio);
        errors.site = Some(SiteId::new("shop").unwrap());
        errors.route = Some(RouteId::new("checkout").unwrap());
        assert_eq!(
            query(&errors),
            "sum(rate(http_server_request_duration_seconds_count{site=\"shop\",route=\"checkout\",\
             http_response_status_code=~\"5..\"}[5m])) / \
             sum(rate(http_server_request_duration_seconds_count\
             {site=\"shop\",route=\"checkout\"}[5m]))"
        );
        assert_eq!(
            query(&rule(Measure::RequestRate)),
            "sum(rate(http_server_request_duration_seconds_count[5m])) or vector(0)"
        );
        let mut upstream = rule(Measure::UpstreamErrorRatio);
        upstream.upstream = Some(UpstreamPoolId::new("app").unwrap());
        assert_eq!(
            query(&upstream),
            "sum(rate(http_client_request_duration_seconds_count{upstream=\"app\",\
             error_type!=\"\"}[5m])) / \
             sum(rate(http_client_request_duration_seconds_count{upstream=\"app\"}[5m]))"
        );
        assert_eq!(
            query(&rule(Measure::LatencyP95)),
            "histogram_quantile(0.95, sum by (le) \
             (rate(http_server_request_duration_seconds_bucket[5m])))"
        );
    }
}
