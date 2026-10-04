//! `pingora.panel.observability.v1.Traffic` over Prometheus (ADR 0022).

use crate::promql::{Queries, Scope, ACTIVATED_AT, OPEN_CONNECTIONS, REVISION};
use panel_contracts::observability::v1::{self as wire, traffic_server::Traffic};
use panel_errors::{PanelError, Result};
use prometheus_http_query::{response::Data, Client};
use std::{
    collections::{BTreeMap, HashMap},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tonic::{Request, Response, Status};

const DEFAULT_WINDOW: Duration = Duration::from_secs(3600);
const MIN_WINDOW: Duration = Duration::from_secs(60);
const MAX_WINDOW: Duration = Duration::from_secs(31 * 86_400);
/// The scrape interval; a rate needs at least two samples.
const MIN_STEP: Duration = Duration::from_secs(15);
const DEFAULT_POINTS: u32 = 120;
const MAX_POINTS: u32 = 720;
const ROUTES: usize = 20;
const DOMAINS: usize = 20;
const QUANTILES: [f64; 4] = [0.5, 0.9, 0.95, 0.99];

pub struct TrafficService {
    prometheus: Client,
}

/// Prometheus's answers: a value, or values by one label.
type Values = Vec<(HashMap<String, String>, f64)>;

impl TrafficService {
    pub fn new(prometheus: Client) -> Self {
        Self { prometheus }
    }

    async fn instant(&self, query: String) -> Result<Values> {
        let result = self
            .prometheus
            .query(&query)
            .get()
            .await
            .map_err(unavailable)?;
        Ok(match result.data() {
            Data::Vector(vector) => vector
                .iter()
                .map(|sample| (sample.metric().clone(), sample.sample().value()))
                .collect(),
            Data::Scalar(sample) => vec![(HashMap::new(), sample.value())],
            Data::Matrix(_) => Vec::new(),
        })
    }

    async fn value(&self, query: String) -> Result<Option<f64>> {
        Ok(self
            .instant(query)
            .await?
            .first()
            .map(|(_, value)| *value)
            .filter(|value| value.is_finite()))
    }

    async fn by(&self, query: String, label: &str) -> Result<BTreeMap<String, f64>> {
        Ok(self
            .instant(query)
            .await?
            .into_iter()
            .filter(|(_, value)| value.is_finite())
            .filter_map(|(mut labels, value)| Some((labels.remove(label)?, value)))
            .collect())
    }

    async fn latency(&self, queries: &Queries) -> Result<wire::Latency> {
        let [p50, p90, p95, p99] = QUANTILES.map(|quantile| self.value(queries.latency(quantile)));
        let (p50, p90, p95, p99) = tokio::try_join!(p50, p90, p95, p99)?;
        Ok(wire::Latency { p50, p90, p95, p99 })
    }

    async fn upstream_latency(&self, queries: &Queries) -> Result<BTreeMap<String, wire::Latency>> {
        let [p50, p90, p95, p99] =
            QUANTILES.map(|quantile| self.by(queries.upstream_latency(quantile), "upstream"));
        let (p50, p90, p95, p99) = tokio::try_join!(p50, p90, p95, p99)?;
        let mut latency: BTreeMap<String, wire::Latency> = BTreeMap::new();
        for (quantile, values) in [p50, p90, p95, p99].into_iter().enumerate() {
            for (upstream, value) in values {
                let entry = latency.entry(upstream).or_default();
                let slot = match quantile {
                    0 => &mut entry.p50,
                    1 => &mut entry.p90,
                    2 => &mut entry.p95,
                    _ => &mut entry.p99,
                };
                *slot = Some(value);
            }
        }
        Ok(latency)
    }

    async fn routes(&self, queries: &Queries) -> Result<Vec<wire::RouteTraffic>> {
        let mut routes: Vec<wire::RouteTraffic> = self
            .instant(queries.routes(ROUTES))
            .await?
            .into_iter()
            .filter(|(_, requests)| requests.is_finite())
            .filter_map(|(mut labels, requests)| {
                Some(wire::RouteTraffic {
                    site: labels.remove("site")?,
                    route: labels.remove("route")?,
                    requests,
                })
            })
            .collect();
        routes.sort_by(|left, right| right.requests.total_cmp(&left.requests));
        Ok(routes)
    }

    async fn domains(&self, queries: &Queries) -> Result<Vec<wire::DomainTraffic>> {
        let mut domains: Vec<wire::DomainTraffic> = self
            .instant(queries.domains(DOMAINS))
            .await?
            .into_iter()
            .filter(|(_, requests)| requests.is_finite())
            .filter_map(|(mut labels, requests)| {
                Some(wire::DomainTraffic {
                    site: labels.remove("site")?,
                    domain: labels.remove("domain")?,
                    requests,
                })
            })
            .collect();
        domains.sort_by(|left, right| right.requests.total_cmp(&left.requests));
        Ok(domains)
    }

    pub async fn summarize(&self, scope: Scope, window: Duration) -> Result<wire::Summary> {
        let queries = Queries::new(scope, window);
        let (
            requests,
            requests_per_second,
            classes,
            latency,
            bytes_received,
            bytes_sent,
            open_connections,
            tls_handshakes,
        ) = tokio::try_join!(
            self.value(queries.requests()),
            self.value(queries.requests_per_second()),
            self.by(queries.status_classes(), "class"),
            self.latency(&queries),
            self.value(queries.bytes_received()),
            self.value(queries.bytes_sent()),
            self.value(OPEN_CONNECTIONS.to_owned()),
            self.value(queries.tls_handshakes()),
        )?;
        let (attempts, failures, upstream_latency, routes, domains, revision, activated_at) = tokio::try_join!(
            self.by(queries.upstream_requests(false), "upstream"),
            self.by(queries.upstream_requests(true), "upstream"),
            self.upstream_latency(&queries),
            self.routes(&queries),
            self.domains(&queries),
            self.value(REVISION.to_owned()),
            self.value(ACTIVATED_AT.to_owned()),
        )?;
        let class = |digit: &str| classes.get(digit).copied().unwrap_or_default();
        let mut upstreams: Vec<wire::UpstreamTraffic> = attempts
            .into_iter()
            .map(|(upstream, requests)| wire::UpstreamTraffic {
                error_ratio: match requests {
                    0.0 => 0.0,
                    _ => failures.get(&upstream).copied().unwrap_or_default() / requests,
                },
                latency: upstream_latency.get(&upstream).cloned(),
                upstream,
                requests,
            })
            .collect();
        upstreams.sort_by(|left, right| right.requests.total_cmp(&left.requests));
        Ok(wire::Summary {
            observed_at: Some(SystemTime::now().into()),
            window: prost_types::Duration::try_from(window).ok(),
            requests: requests.unwrap_or_default(),
            requests_per_second: requests_per_second.unwrap_or_default(),
            statuses: Some(wire::StatusClasses {
                informational: class("1"),
                success: class("2"),
                redirection: class("3"),
                client_error: class("4"),
                server_error: class("5"),
            }),
            latency: Some(latency),
            bytes_received: bytes_received.unwrap_or_default(),
            bytes_sent: bytes_sent.unwrap_or_default(),
            open_connections: open_connections.unwrap_or_default(),
            tls_handshakes: tls_handshakes.unwrap_or_default(),
            upstreams,
            routes,
            domains,
            revision: revision.and_then(|revision| {
                (revision >= 0.0 && revision <= u64::MAX as f64).then_some(revision as u64)
            }),
            activated_at: activated_at.map(seconds_since_epoch),
        })
    }

    async fn range(
        &self,
        query: String,
        start: i64,
        end: i64,
        step: Duration,
    ) -> Result<BTreeMap<i64, f64>> {
        let result = self
            .prometheus
            .query_range(&query, start, end, step.as_secs_f64())
            .get()
            .await
            .map_err(unavailable)?;
        let Data::Matrix(series) = result.data() else {
            return Ok(BTreeMap::new());
        };
        Ok(series
            .first()
            .map(|series| {
                series
                    .samples()
                    .iter()
                    .filter(|sample| sample.value().is_finite())
                    .map(|sample| (sample.timestamp() as i64, sample.value()))
                    .collect()
            })
            .unwrap_or_default())
    }

    pub async fn chart(
        &self,
        scope: Scope,
        window: Duration,
        step: Duration,
    ) -> Result<Vec<wire::Point>> {
        let end = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let start = end.saturating_sub(window.as_secs());
        let (start, end) = (
            i64::try_from(start).unwrap_or(i64::MAX),
            i64::try_from(end).unwrap_or(i64::MAX),
        );
        // Each point's rate covers its step and at least four scrapes.
        let queries = Queries::new(scope, step.max(4 * MIN_STEP));
        let (rates, errors, p95) = tokio::try_join!(
            self.range(queries.requests_per_second(), start, end, step),
            self.range(queries.server_errors_per_second(), start, end, step),
            self.range(queries.latency(0.95), start, end, step),
        )?;
        Ok(rates
            .into_iter()
            .map(|(at, requests_per_second)| wire::Point {
                at: Some(prost_types::Timestamp {
                    seconds: at,
                    nanos: 0,
                }),
                requests_per_second,
                server_errors_per_second: errors.get(&at).copied().unwrap_or_default(),
                p95: p95.get(&at).copied(),
            })
            .collect())
    }
}

fn unavailable(error: prometheus_http_query::Error) -> PanelError {
    PanelError::unavailable(format!("Prometheus did not answer: {error}"))
}

fn seconds_since_epoch(seconds: f64) -> prost_types::Timestamp {
    let whole = seconds.trunc();
    prost_types::Timestamp {
        seconds: whole as i64,
        nanos: ((seconds - whole) * 1e9) as i32,
    }
}

/// The requested window, within bounds; an hour when absent.
fn window(requested: Option<prost_types::Duration>) -> Result<Duration> {
    let Some(requested) = requested else {
        return Ok(DEFAULT_WINDOW);
    };
    let window = Duration::try_from(requested)
        .map_err(|_| PanelError::invalid_argument("a window must be a positive duration"))?;
    Ok(window.clamp(MIN_WINDOW, MAX_WINDOW))
}

/// The requested step, so that a window has at most `MAX_POINTS` points.
fn step(requested: Option<prost_types::Duration>, window: Duration) -> Result<Duration> {
    let fewest = (window / MAX_POINTS).max(MIN_STEP);
    let step = match requested {
        Some(requested) => Duration::try_from(requested)
            .map_err(|_| PanelError::invalid_argument("a step must be a positive duration"))?,
        None => window / DEFAULT_POINTS,
    };
    Ok(Duration::from_secs(step.max(fewest).as_secs().max(1)))
}

fn scope(scope: Option<wire::Scope>) -> Result<Scope> {
    let scope = scope.unwrap_or_default();
    Scope::new(&scope.site, &scope.route)
}

#[tonic::async_trait]
impl Traffic for TrafficService {
    async fn summary(
        &self,
        request: Request<wire::SummaryRequest>,
    ) -> std::result::Result<Response<wire::SummaryResponse>, Status> {
        let request = request.into_inner();
        let result = async {
            let window = window(request.window)?;
            self.summarize(scope(request.scope)?, window).await
        }
        .await;
        Ok(Response::new(match result {
            Ok(summary) => wire::SummaryResponse {
                summary: Some(summary),
                error: None,
            },
            Err(error) => wire::SummaryResponse {
                summary: None,
                error: Some(error.into()),
            },
        }))
    }

    async fn series(
        &self,
        request: Request<wire::SeriesRequest>,
    ) -> std::result::Result<Response<wire::SeriesResponse>, Status> {
        let request = request.into_inner();
        let result = async {
            let window = window(request.window)?;
            let step = step(request.step, window)?;
            self.chart(scope(request.scope)?, window, step).await
        }
        .await;
        Ok(Response::new(match result {
            Ok(points) => wire::SeriesResponse {
                points,
                error: None,
            },
            Err(error) => wire::SeriesResponse {
                points: Vec::new(),
                error: Some(error.into()),
            },
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_and_steps_are_bounded() {
        assert_eq!(window(None).unwrap(), DEFAULT_WINDOW);
        let seconds = |seconds| Some(prost_types::Duration { seconds, nanos: 0 });
        assert_eq!(window(seconds(1)).unwrap(), MIN_WINDOW);
        assert_eq!(window(seconds(i64::MAX)).unwrap(), MAX_WINDOW);
        assert!(window(seconds(-1)).is_err());
        let day = Duration::from_secs(86_400);
        assert_eq!(step(None, day).unwrap(), Duration::from_secs(720));
        assert_eq!(step(seconds(1), day).unwrap(), Duration::from_secs(120));
        assert_eq!(
            step(None, Duration::from_secs(600)).unwrap(),
            MIN_STEP,
            "{MIN_STEP:?}"
        );
    }
}
