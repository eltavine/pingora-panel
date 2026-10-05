#![forbid(unsafe_code)]

//! Traffic summaries and series from a stand-in for the Prometheus HTTP API.

use axum::{extract::Query, routing::get, Json, Router};
use observability_service::{prometheus, Scope, TrafficService};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

type Seen = Arc<Mutex<Vec<String>>>;

fn sample(labels: &[(&str, &str)], value: &str) -> serde_json::Value {
    let metric: serde_json::Map<String, serde_json::Value> = labels
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).into()))
        .collect();
    serde_json::json!({ "metric": metric, "value": [1_700_000_000, value] })
}

/// What Prometheus would answer, by what the query asks.
fn answer(query: &str) -> Vec<serde_json::Value> {
    let one = |value| vec![sample(&[], value)];
    let handler = |route, phase| [("site", "shop"), ("route", route), ("phase", phase)];
    if query.contains("pingora_panel_gateway_lua_runs_total") && query.contains("outcome!=") {
        if query.starts_with("topk") {
            vec![sample(&handler("checkout", "access"), "3")]
        } else {
            vec![
                sample(&[("outcome", "timeout")], "2"),
                sample(&[("outcome", "error")], "1"),
            ]
        }
    } else if query.contains("pingora_panel_gateway_lua_runs_total") {
        if query.starts_with("topk") {
            vec![
                sample(&handler("home", "log"), "80"),
                sample(&handler("checkout", "access"), "20"),
            ]
        } else {
            one("100")
        }
    } else if query.contains("pingora_panel_gateway_lua_slow_runs_total") {
        if query.contains("by (site") {
            vec![sample(&handler("checkout", "access"), "4")]
        } else {
            one("4")
        }
    } else if query.contains("pingora_panel_gateway_lua_run_duration_seconds_bucket") {
        if query.contains("by (site") {
            vec![sample(&handler("checkout", "access"), "0.012")]
        } else {
            one("0.003")
        }
    } else if query.contains("pingora_panel_gateway_lua_memory_bytes") {
        one("1048576")
    } else if query.contains("label_replace") {
        vec![
            sample(&[("class", "2")], "90"),
            sample(&[("class", "5")], "10"),
        ]
    } else if query.contains("sum by (upstream, server_address, server_port, error_type)") {
        vec![
            sample(
                &[
                    ("upstream", "app"),
                    ("server_address", "10.0.0.7"),
                    ("server_port", "8080"),
                    ("error_type", "connect_refused"),
                ],
                "4",
            ),
            sample(
                &[
                    ("upstream", "app"),
                    ("server_address", "10.0.0.8"),
                    ("server_port", "8080"),
                    ("error_type", "504"),
                ],
                "1",
            ),
        ]
    } else if query.contains("pingora_panel_gateway_upstream_connections_total{reused") {
        vec![sample(&[("upstream", "app")], "30")]
    } else if query.contains("pingora_panel_gateway_upstream_connections_total") {
        vec![sample(&[("upstream", "app")], "40")]
    } else if query.contains("pingora_panel_gateway_domain_requests_total") {
        vec![
            sample(&[("site", "shop"), ("domain", "*.shop.example")], "30"),
            sample(&[("site", "shop"), ("domain", "shop.example")], "70"),
        ]
    } else if query.starts_with("topk(20") {
        vec![
            sample(&[("site", "shop"), ("route", "home")], "40"),
            sample(&[("site", "shop"), ("route", "checkout")], "60"),
        ]
    } else if query.contains("http_client_request_duration_seconds_count{error_type") {
        vec![sample(&[("upstream", "app")], "5")]
    } else if query.contains("http_client_request_duration_seconds_count") {
        vec![sample(&[("upstream", "app")], "50")]
    } else if query.contains("http_client_request_duration_seconds_bucket") {
        vec![sample(&[("upstream", "app")], "0.1")]
    } else if query.starts_with("histogram_quantile(0.99") {
        one("NaN")
    } else if query.starts_with("histogram_quantile") {
        one("0.25")
    } else if query.starts_with("sum(increase(http_server_request_duration_seconds_count") {
        one("100")
    } else if query.starts_with("sum(rate(") {
        one("0.5")
    } else if query.contains("request_body_size") {
        one("2048")
    } else if query.contains("response_body_size") {
        one("4096")
    } else if query.contains("open_connections") {
        one("3")
    } else if query.contains("tls_handshakes") {
        one("7")
    } else if query.contains("config_revision") {
        one("42")
    } else if query.contains("activated_timestamp") {
        one("1700000000.5")
    } else {
        Vec::new()
    }
}

async fn prometheus_stand_in(seen: Seen) -> String {
    let instant = {
        let seen = Arc::clone(&seen);
        move |Query(params): Query<HashMap<String, String>>| {
            let seen = Arc::clone(&seen);
            async move {
                let query = params.get("query").cloned().unwrap_or_default();
                let result = answer(&query);
                seen.lock().unwrap().push(query);
                Json(serde_json::json!({
                    "status": "success",
                    "data": { "resultType": "vector", "result": result },
                }))
            }
        }
    };
    let range = move |Query(params): Query<HashMap<String, String>>| {
        let seen = Arc::clone(&seen);
        async move {
            let query = params.get("query").cloned().unwrap_or_default();
            seen.lock().unwrap().push(query.clone());
            let value = if query.contains("5..") { "0.1" } else { "2" };
            Json(serde_json::json!({
                "status": "success",
                "data": {
                    "resultType": "matrix",
                    "result": [{
                        "metric": {},
                        "values": [[1_700_000_000, value], [1_700_000_060, value]],
                    }],
                },
            }))
        }
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = Router::new()
        .route("/api/v1/query", get(instant))
        .route("/api/v1/query_range", get(range));
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{address}")
}

#[tokio::test]
async fn summaries_gather_every_figure_of_a_scope() {
    let seen = Seen::default();
    let url = prometheus_stand_in(Arc::clone(&seen)).await;
    let traffic = TrafficService::new(prometheus(&url).unwrap());

    let summary = traffic
        .summarize(Scope::new("shop", "").unwrap(), Duration::from_secs(3600))
        .await
        .unwrap();

    assert_eq!(summary.requests, 100.0);
    assert_eq!(summary.requests_per_second, 0.5);
    let statuses = summary.statuses.unwrap();
    assert_eq!((statuses.success, statuses.server_error), (90.0, 10.0));
    let latency = summary.latency.unwrap();
    assert_eq!((latency.p95, latency.p99), (Some(0.25), None));
    assert_eq!(
        (summary.bytes_received, summary.bytes_sent),
        (2048.0, 4096.0)
    );
    assert_eq!(
        (summary.open_connections, summary.tls_handshakes),
        (3.0, 7.0)
    );
    let routes: Vec<_> = summary
        .routes
        .iter()
        .map(|route| route.route.as_str())
        .collect();
    assert_eq!(routes, ["checkout", "home"], "busiest first");
    let failure = &summary.upstream_failures[0];
    assert_eq!(
        (
            failure.upstream.as_str(),
            failure.address.as_str(),
            failure.port,
            failure.error_type.as_str(),
            failure.failures
        ),
        ("app", "10.0.0.7", 8080, "connect_refused", 4.0),
        "most first"
    );
    assert_eq!(summary.upstream_failures.len(), 2);
    let domains: Vec<_> = summary
        .domains
        .iter()
        .map(|domain| (domain.domain.as_str(), domain.requests))
        .collect();
    assert_eq!(
        domains,
        [("shop.example", 70.0), ("*.shop.example", 30.0)],
        "busiest first"
    );
    let upstream = &summary.upstreams[0];
    assert_eq!(
        (upstream.upstream.as_str(), upstream.requests),
        ("app", 50.0)
    );
    assert!((upstream.error_ratio - 0.1).abs() < 1e-9);
    assert_eq!(upstream.latency.as_ref().unwrap().p50, Some(0.1));
    assert_eq!(upstream.connection_reuse_ratio, Some(0.75));
    assert_eq!(summary.revision, Some(42));
    assert_eq!(summary.activated_at.unwrap().seconds, 1_700_000_000);
    let lua = summary.lua.unwrap();
    assert_eq!(
        (lua.runs, lua.slow_runs, lua.memory_bytes),
        (100.0, 4.0, 1_048_576.0)
    );
    assert_eq!((lua.failures["timeout"], lua.failures["error"]), (2.0, 1.0));
    assert_eq!(lua.latency.unwrap().p50, Some(0.003));
    let failing = &lua.handlers[0];
    assert_eq!(
        (
            failing.route.as_str(),
            failing.phase.as_str(),
            failing.failures,
            failing.runs,
            failing.slow_runs,
            failing.p95
        ),
        ("checkout", "access", 3.0, 20.0, 4.0, Some(0.012)),
        "most failures first"
    );
    assert_eq!(lua.handlers[1].route, "home");

    let seen = seen.lock().unwrap();
    assert!(seen.contains(
        &"sum(increase(http_server_request_duration_seconds_count{site=\"shop\"}[3600s]))"
            .to_owned()
    ));
}

#[tokio::test]
async fn series_align_rates_errors_and_latency() {
    let url = prometheus_stand_in(Seen::default()).await;
    let traffic = TrafficService::new(prometheus(&url).unwrap());

    let points = traffic
        .chart(
            Scope::default(),
            Duration::from_secs(3600),
            Duration::from_secs(60),
        )
        .await
        .unwrap();

    assert_eq!(points.len(), 2);
    assert_eq!(points[1].at.as_ref().unwrap().seconds, 1_700_000_060);
    assert_eq!(points[0].requests_per_second, 2.0);
    assert_eq!(points[0].server_errors_per_second, 0.1);
    assert_eq!(points[0].p95, Some(2.0));
}

#[tokio::test]
async fn an_unreachable_prometheus_is_unavailable() {
    let traffic = TrafficService::new(prometheus("http://127.0.0.1:9").unwrap());
    let error = traffic
        .summarize(Scope::default(), Duration::from_secs(60))
        .await
        .unwrap_err();
    assert_eq!(error.code.as_str(), "UNAVAILABLE");
}
