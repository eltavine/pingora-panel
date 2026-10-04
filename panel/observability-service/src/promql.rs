//! The PromQL that answers traffic queries, over the metric names of
//! ADR 0022. A scope names configured sites and routes, which are validated
//! identifiers, and is quoted as PromQL strings besides, so a query never
//! carries anything but what it means.

use panel_domain::{RouteId, SiteId};
use panel_errors::{PanelError, Result};
use std::time::Duration;

const REQUESTS: &str = "http_server_request_duration_seconds_count";
const REQUEST_BUCKETS: &str = "http_server_request_duration_seconds_bucket";
const UPSTREAM_REQUESTS: &str = "http_client_request_duration_seconds_count";
const UPSTREAM_BUCKETS: &str = "http_client_request_duration_seconds_bucket";
const DOMAIN_REQUESTS: &str = "pingora_panel_gateway_domain_requests_total";

/// Which requests a query reads.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Scope {
    site: Option<SiteId>,
    route: Option<RouteId>,
}

impl Scope {
    /// Every request, or those of `site`, or of one of its routes; empty
    /// names are unset.
    pub fn new(site: &str, route: &str) -> Result<Self> {
        let invalid =
            |error: panel_domain::DomainError| PanelError::invalid_argument(error.to_string());
        let site = (!site.is_empty())
            .then(|| SiteId::new(site))
            .transpose()
            .map_err(invalid)?;
        let route = (!route.is_empty())
            .then(|| RouteId::new(route))
            .transpose()
            .map_err(invalid)?;
        if route.is_some() && site.is_none() {
            return Err(PanelError::invalid_argument(
                "a route is read within its site",
            ));
        }
        Ok(Self { site, route })
    }

    /// Only the site: metrics without routes read the whole site.
    fn site_selector(&self) -> String {
        self.site
            .as_ref()
            .map(|site| format!("{{site={}}}", quoted(site.as_str())))
            .unwrap_or_default()
    }

    fn selector(&self, extra: &[&str]) -> String {
        let mut matchers: Vec<String> = Vec::new();
        if let Some(site) = &self.site {
            matchers.push(format!("site={}", quoted(site.as_str())));
        }
        if let Some(route) = &self.route {
            matchers.push(format!("route={}", quoted(route.as_str())));
        }
        matchers.extend(extra.iter().map(|matcher| (*matcher).to_owned()));
        if matchers.is_empty() {
            String::new()
        } else {
            format!("{{{}}}", matchers.join(","))
        }
    }
}

/// A PromQL string literal.
pub(crate) fn quoted(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for character in value.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

/// A PromQL range in whole seconds, at least one.
fn range(duration: Duration) -> String {
    format!("{}s", duration.as_secs().max(1))
}

/// The queries of one scope over one window.
#[derive(Clone, Debug)]
pub struct Queries {
    scope: Scope,
    range: String,
}

impl Queries {
    pub fn new(scope: Scope, window: Duration) -> Self {
        Self {
            scope,
            range: range(window),
        }
    }

    fn requests_over(&self, extra: &[&str], function: &str) -> String {
        format!(
            "sum({function}({REQUESTS}{}[{}]))",
            self.scope.selector(extra),
            self.range
        )
    }

    pub fn requests(&self) -> String {
        self.requests_over(&[], "increase")
    }

    pub fn requests_per_second(&self) -> String {
        self.requests_over(&[], "rate")
    }

    pub fn server_errors_per_second(&self) -> String {
        self.requests_over(&["http_response_status_code=~\"5..\""], "rate")
    }

    /// Requests by the first digit of their status code, as `class`.
    pub fn status_classes(&self) -> String {
        format!(
            "sum by (class) (label_replace(increase({REQUESTS}{}[{}]), \"class\", \"$1\", \
             \"http_response_status_code\", \"([1-5])[0-9][0-9]\"))",
            self.scope.selector(&[]),
            self.range
        )
    }

    pub fn latency(&self, quantile: f64) -> String {
        format!(
            "histogram_quantile({quantile}, sum by (le) (rate({REQUEST_BUCKETS}{}[{}])))",
            self.scope.selector(&[]),
            self.range
        )
    }

    pub fn bytes_received(&self) -> String {
        format!(
            "sum(increase(http_server_request_body_size_bytes_sum{}[{}]))",
            self.scope.selector(&[]),
            self.range
        )
    }

    pub fn bytes_sent(&self) -> String {
        format!(
            "sum(increase(http_server_response_body_size_bytes_sum{}[{}]))",
            self.scope.selector(&[]),
            self.range
        )
    }

    /// The busiest routes, as `site` and `route`.
    pub fn routes(&self, limit: usize) -> String {
        format!(
            "topk({limit}, sum by (site, route) (increase({REQUESTS}{}[{}])))",
            self.scope.selector(&["route!=\"\""]),
            self.range
        )
    }

    /// The busiest configured domains, as `site` and `domain`; a route's
    /// scope reads its site's.
    pub fn domains(&self, limit: usize) -> String {
        format!(
            "topk({limit}, sum by (site, domain) (increase({DOMAIN_REQUESTS}{}[{}])))",
            self.scope.site_selector(),
            self.range
        )
    }

    pub fn tls_handshakes(&self) -> String {
        format!(
            "sum(increase(pingora_panel_gateway_tls_handshakes_total[{}]))",
            self.range
        )
    }

    /// Attempts by `upstream`; with `failed`, only the failed ones.
    pub fn upstream_requests(&self, failed: bool) -> String {
        let selector = if failed { "{error_type!=\"\"}" } else { "" };
        format!(
            "sum by (upstream) (increase({UPSTREAM_REQUESTS}{selector}[{}]))",
            self.range
        )
    }

    /// Failed attempts by upstream node and why, most first.
    pub fn upstream_failures(&self, limit: usize) -> String {
        format!(
            "topk({limit}, sum by (upstream, server_address, server_port, error_type) \
             (increase({UPSTREAM_REQUESTS}{{error_type!=\"\"}}[{}])))",
            self.range
        )
    }

    pub fn upstream_latency(&self, quantile: f64) -> String {
        format!(
            "histogram_quantile({quantile}, sum by (upstream, le) (rate({UPSTREAM_BUCKETS}[{}])))",
            self.range
        )
    }
}

pub const OPEN_CONNECTIONS: &str = "sum(pingora_panel_gateway_open_connections)";
pub const REVISION: &str = "max(pingora_panel_gateway_config_revision)";
pub const ACTIVATED_AT: &str = "max(pingora_panel_gateway_config_activated_timestamp_seconds)";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_select_sites_and_routes() {
        let queries = Queries::new(
            Scope::new("shop", "checkout").unwrap(),
            Duration::from_secs(3600),
        );
        assert_eq!(
            queries.requests(),
            "sum(increase(http_server_request_duration_seconds_count\
             {site=\"shop\",route=\"checkout\"}[3600s]))"
        );
        assert_eq!(
            queries.server_errors_per_second(),
            "sum(rate(http_server_request_duration_seconds_count\
             {site=\"shop\",route=\"checkout\",http_response_status_code=~\"5..\"}[3600s]))"
        );
        let everything = Queries::new(Scope::default(), Duration::from_secs(300));
        assert_eq!(
            everything.latency(0.95),
            "histogram_quantile(0.95, sum by (le) \
             (rate(http_server_request_duration_seconds_bucket[300s])))"
        );
        assert_eq!(
            everything.routes(20),
            "topk(20, sum by (site, route) \
             (increase(http_server_request_duration_seconds_count{route!=\"\"}[300s])))"
        );
        assert_eq!(
            everything.domains(20),
            "topk(20, sum by (site, domain) \
             (increase(pingora_panel_gateway_domain_requests_total[300s])))"
        );
        assert_eq!(
            everything.upstream_failures(20),
            "topk(20, sum by (upstream, server_address, server_port, error_type) \
             (increase(http_client_request_duration_seconds_count{error_type!=\"\"}[300s])))"
        );
        assert_eq!(
            queries.domains(20),
            "topk(20, sum by (site, domain) \
             (increase(pingora_panel_gateway_domain_requests_total{site=\"shop\"}[3600s])))"
        );
    }

    #[test]
    fn scopes_name_identifiers_only() {
        assert!(Scope::new("shop\"}) or vector(1", "").is_err());
        assert!(Scope::new("", "checkout").is_err());
        assert_eq!(quoted("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }
}
