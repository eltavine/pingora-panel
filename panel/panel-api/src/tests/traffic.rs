use super::*;
use panel_application::{
    DomainTraffic, Latency, RequestScope, RouteTraffic, StatusClasses, TrafficPoint, TrafficPort,
    TrafficQuery, TrafficSummary, UpstreamFailure, UpstreamTraffic,
};
use panel_domain::{RouteId, SiteId};
use serde_json::{json, Value};
use std::{
    sync::Mutex,
    time::{Duration, UNIX_EPOCH},
};

#[derive(Default)]
struct Traffic {
    queries: Mutex<Vec<(TrafficQuery, Option<Duration>)>>,
}

#[async_trait]
impl TrafficPort for Traffic {
    async fn summary(&self, _scope: RequestScope, query: TrafficQuery) -> Result<TrafficSummary> {
        self.queries.lock().unwrap().push((query, None));
        Ok(TrafficSummary {
            observed_at: Some(UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
            window: Duration::from_secs(3600),
            requests: 120.0,
            requests_per_second: 0.5,
            statuses: StatusClasses {
                success: 110.0,
                server_error: 10.0,
                ..StatusClasses::default()
            },
            latency: Latency {
                p95: Some(Duration::from_millis(250)),
                ..Latency::default()
            },
            upstreams: vec![UpstreamTraffic {
                upstream: "app".into(),
                requests: 50.0,
                error_ratio: 0.1,
                latency: Latency::default(),
            }],
            routes: vec![RouteTraffic {
                site: "shop".into(),
                route: "checkout".into(),
                requests: 60.0,
            }],
            upstream_failures: vec![UpstreamFailure {
                upstream: "app".into(),
                address: "10.0.0.7".into(),
                port: 8080,
                error_type: "connect_refused".into(),
                failures: 4.0,
            }],
            domains: vec![DomainTraffic {
                site: "shop".into(),
                domain: "*.shop.example".into(),
                requests: 30.0,
            }],
            revision: Some(7),
            ..TrafficSummary::default()
        })
    }

    async fn series(
        &self,
        _scope: RequestScope,
        query: TrafficQuery,
        step: Option<Duration>,
    ) -> Result<Vec<TrafficPoint>> {
        self.queries.lock().unwrap().push((query, step));
        Ok(vec![TrafficPoint {
            at: UNIX_EPOCH + Duration::from_secs(1_800_000_000),
            requests_per_second: 2.0,
            server_errors_per_second: 0.1,
            p95: None,
        }])
    }
}

fn traffic_app(traffic: Arc<Traffic>) -> axum::Router {
    router(
        ApiState::new(Arc::new(GatewayService::new(
            Arc::new(FakeGateway),
            Arc::new(IdentityCompiler),
        )))
        .with_traffic(traffic),
    )
}

async fn get(app: &axum::Router, uri: &str) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}

#[tokio::test]
async fn summaries_read_one_scope_over_a_window() {
    let traffic = Arc::new(Traffic::default());
    let app = traffic_app(Arc::clone(&traffic));

    let (status, summary) = get(&app, "/api/v1/traffic?site=shop&route=checkout&window=600").await;
    assert_eq!(status, StatusCode::OK, "{summary}");
    assert_eq!(summary["requests"], 120.0);
    assert_eq!(summary["window_seconds"], 3600);
    assert_eq!(summary["statuses"]["server_error"], 10.0);
    assert_eq!(summary["latency"]["p95"], 0.25);
    assert_eq!(summary["latency"]["p50"], Value::Null);
    assert_eq!(summary["observed_at"], "2027-01-15T08:00:00Z");
    assert_eq!(summary["upstreams"][0]["error_ratio"], 0.1);
    assert_eq!(
        summary["routes"],
        json!([{"site": "shop", "route": "checkout", "requests": 60.0}])
    );
    assert_eq!(
        summary["upstream_failures"],
        json!([{
            "upstream": "app",
            "address": "10.0.0.7",
            "port": 8080,
            "error_type": "connect_refused",
            "failures": 4.0
        }])
    );
    assert_eq!(
        summary["domains"],
        json!([{"site": "shop", "domain": "*.shop.example", "requests": 30.0}])
    );
    assert_eq!(summary["revision"], 7);
    assert_eq!(
        traffic.queries.lock().unwrap()[0],
        (
            TrafficQuery {
                site: Some(SiteId::new("shop").unwrap()),
                route: Some(RouteId::new("checkout").unwrap()),
                window: Some(Duration::from_secs(600)),
            },
            None
        )
    );
}

#[tokio::test]
async fn series_pass_their_step_and_refuse_odd_names() {
    let traffic = Arc::new(Traffic::default());
    let app = traffic_app(Arc::clone(&traffic));

    let (status, series) = get(&app, "/api/v1/traffic/series?window=86400&step=300").await;
    assert_eq!(status, StatusCode::OK, "{series}");
    assert_eq!(
        series["points"],
        json!([{
            "at": "2027-01-15T08:00:00Z",
            "requests_per_second": 2.0,
            "server_errors_per_second": 0.1,
            "p95": null
        }])
    );
    assert_eq!(
        traffic.queries.lock().unwrap()[0].1,
        Some(Duration::from_secs(300))
    );

    let (status, problem) = get(&app, "/api/v1/traffic?site=not%20an%20id").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{problem}");
}

#[tokio::test]
async fn without_a_source_traffic_is_unavailable() {
    let app = router(ApiState::new(Arc::new(GatewayService::new(
        Arc::new(FakeGateway),
        Arc::new(IdentityCompiler),
    ))));
    let (status, _) = get(&app, "/api/v1/traffic").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}
