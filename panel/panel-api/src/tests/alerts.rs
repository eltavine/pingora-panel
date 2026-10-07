use super::*;
use axum::http::HeaderMap;
use panel_application::{
    AlertChannel, AlertChannelKind, AlertChannelSecret, AlertComparison, AlertMeasure,
    AlertNotification, AlertNotificationKind, AlertNotificationQuery, AlertNotificationState,
    AlertRule, AlertRuleSpec, AlertSeverity, AlertState, AlertTest, AlertsPort, NewAlertChannel,
    RequestScope,
};
use panel_domain::SiteId;
use serde_json::{json, Value};
use std::{
    sync::Mutex,
    time::{Duration, UNIX_EPOCH},
};
use zeroize::Zeroizing;

fn spec() -> AlertRuleSpec {
    AlertRuleSpec {
        name: "Shop errors".into(),
        description: String::new(),
        measure: AlertMeasure::ServerErrorRatio,
        comparison: AlertComparison::Above,
        threshold: 0.1,
        pending_for: Duration::from_secs(300),
        site: Some(SiteId::new("shop").unwrap()),
        route: None,
        upstream: None,
        severity: AlertSeverity::Critical,
        enabled: true,
        channels: vec!["ops".into()],
    }
}

fn rule(version: u64) -> AlertRule {
    AlertRule {
        id: "shop-errors".into(),
        spec: spec(),
        version,
        created_at: UNIX_EPOCH + Duration::from_secs(1_800_000_000),
        updated_at: UNIX_EPOCH + Duration::from_secs(1_800_000_000),
        state: AlertState::Firing,
        since: Some(UNIX_EPOCH + Duration::from_secs(1_800_000_060)),
        value: Some(0.25),
        evaluated_at: Some(UNIX_EPOCH + Duration::from_secs(1_800_000_090)),
        evaluation_error: String::new(),
    }
}

fn channel(version: u64) -> AlertChannel {
    AlertChannel {
        id: "ops".into(),
        kind: AlertChannelKind::Webhook,
        target: "https://hooks.example".into(),
        version,
        created_at: UNIX_EPOCH + Duration::from_secs(1_800_000_000),
        updated_at: UNIX_EPOCH + Duration::from_secs(1_800_000_000),
    }
}

#[derive(Default)]
struct Alerts {
    calls: Mutex<Vec<String>>,
}

impl Alerts {
    fn called(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }
}

#[async_trait]
impl AlertsPort for Alerts {
    async fn rules(&self, _scope: RequestScope) -> Result<Vec<AlertRule>> {
        Ok(vec![rule(3)])
    }

    async fn put_rule(
        &self,
        context: CommandContext,
        id: &str,
        spec: AlertRuleSpec,
        version: Option<u64>,
    ) -> Result<AlertRule> {
        self.called(format!(
            "put {id} {version:?} by {} at {}",
            context.actor(),
            spec.threshold
        ));
        Ok(rule(version.map_or(1, |version| version + 1)))
    }

    async fn delete_rule(&self, _context: CommandContext, id: &str, version: u64) -> Result<()> {
        self.called(format!("delete rule {id} {version}"));
        Ok(())
    }

    async fn channels(&self, _scope: RequestScope) -> Result<Vec<AlertChannel>> {
        Ok(vec![channel(1)])
    }

    async fn create_channel(
        &self,
        _context: CommandContext,
        channel_: NewAlertChannel,
    ) -> Result<AlertChannelSecret> {
        let plugin = channel_
            .plugin
            .map(|plugin| {
                format!(
                    " {:?} via {}/{}",
                    channel_.kind, plugin.name, plugin.channel
                )
            })
            .unwrap_or_default();
        self.called(format!("create {} {}{plugin}", channel_.id, *channel_.url));
        Ok(AlertChannelSecret {
            channel: channel(1),
            secret: Zeroizing::new("whsec_c2VjcmV0".into()),
        })
    }

    async fn rotate_channel(
        &self,
        _context: CommandContext,
        id: &str,
        url: Option<Zeroizing<String>>,
        version: u64,
    ) -> Result<AlertChannelSecret> {
        self.called(format!(
            "rotate {id} {:?} {version}",
            url.as_deref().map(String::as_str)
        ));
        Ok(AlertChannelSecret {
            channel: channel(version + 1),
            secret: Zeroizing::new("whsec_bmV3".into()),
        })
    }

    async fn delete_channel(&self, _context: CommandContext, id: &str, version: u64) -> Result<()> {
        self.called(format!("delete channel {id} {version}"));
        Ok(())
    }

    async fn test_channel(&self, _context: CommandContext, id: &str) -> Result<AlertTest> {
        self.called(format!("test {id}"));
        Ok(AlertTest {
            delivered: false,
            status: Some(503),
            failure: "the receiver answered 503".into(),
        })
    }

    async fn notifications(
        &self,
        _scope: RequestScope,
        query: AlertNotificationQuery,
    ) -> Result<Vec<AlertNotification>> {
        self.called(format!("notifications {query:?}"));
        Ok(vec![AlertNotification {
            id: "0192".into(),
            rule: "shop-errors".into(),
            channel: "ops".into(),
            kind: AlertNotificationKind::Resolved,
            state: AlertNotificationState::Queued,
            attempts: 2,
            created_at: UNIX_EPOCH + Duration::from_secs(1_800_000_000),
            next_attempt_at: Some(UNIX_EPOCH + Duration::from_secs(1_800_000_030)),
            delivered_at: None,
            last_failure: "the receiver answered 503".into(),
        }])
    }
}

fn alerts_app(alerts: Arc<Alerts>) -> axum::Router {
    router(
        ApiState::new(Arc::new(GatewayService::new(
            Arc::new(FakeGateway),
            Arc::new(IdentityCompiler),
        )))
        .with_alerts(alerts),
    )
}

fn command(method: &str, uri: &str) -> axum::http::request::Builder {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("x-actor", "ops")
        .header("x-deadline", "2099-01-01T00:00:00Z")
        .header("idempotency-key", format!("{method}-{uri}"))
}

async fn send(app: &axum::Router, request: Request<Body>) -> (StatusCode, HeaderMap, Value) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        headers,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn json_body(value: Value) -> Body {
    Body::from(value.to_string())
}

#[tokio::test]
async fn rules_are_listed_with_their_state_and_put_by_version() {
    let alerts = Arc::new(Alerts::default());
    let app = alerts_app(Arc::clone(&alerts));

    let (status, _, listed) = send(
        &app,
        Request::get("/api/v1/alert-rules")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(
        listed[0],
        json!({
            "id": "shop-errors",
            "spec": {
                "name": "Shop errors", "description": "", "measure": "server_error_ratio",
                "comparison": "above", "threshold": 0.1, "pending_seconds": 300,
                "site": "shop", "route": null, "upstream": null, "severity": "critical",
                "enabled": true, "channels": ["ops"]
            },
            "version": 3,
            "etag": "\"3\"",
            "created_at": "2027-01-15T08:00:00Z",
            "updated_at": "2027-01-15T08:00:00Z",
            "state": "firing",
            "since": "2027-01-15T08:01:00Z",
            "value": 0.25,
            "evaluated_at": "2027-01-15T08:01:30Z",
            "evaluation_error": null
        })
    );

    let body = json!({
        "name": "Shop errors", "measure": "server_error_ratio", "comparison": "above",
        "threshold": 0.2, "severity": "critical", "site": "shop", "channels": ["ops"]
    });
    let (status, headers, created) = send(
        &app,
        command("PUT", "/api/v1/alert-rules/shop-errors")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(body.clone()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(headers[header::ETAG], "\"1\"");
    let (status, headers, _) = send(
        &app,
        command("PUT", "/api/v1/alert-rules/shop-errors")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::IF_MATCH, "\"3\"")
            .body(json_body(body.clone()))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::ETAG], "\"4\"");
    let (status, _, _) = send(
        &app,
        command("PUT", "/api/v1/alert-rules/shop-errors")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::IF_MATCH, "W/\"3\"")
            .body(json_body(body))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _, problem) = send(
        &app,
        command("DELETE", "/api/v1/alert-rules/shop-errors")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::PRECONDITION_REQUIRED, "{problem}");
    let (status, _, _) = send(
        &app,
        command("DELETE", "/api/v1/alert-rules/shop-errors")
            .header(header::IF_MATCH, "\"4\"")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        *alerts.calls.lock().unwrap(),
        [
            "put shop-errors None by ops at 0.2",
            "put shop-errors Some(3) by ops at 0.2",
            "delete rule shop-errors 4",
        ]
    );
}

#[tokio::test]
async fn channels_return_their_secret_once_and_are_tested() {
    let alerts = Arc::new(Alerts::default());
    let app = alerts_app(Arc::clone(&alerts));

    let (status, headers, created) = send(
        &app,
        command("POST", "/api/v1/alert-channels")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(
                json!({"id": "ops", "url": "https://hooks.example/T0/secret"}),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(created["secret"], "whsec_c2VjcmV0");
    assert_eq!(created["channel"]["target"], "https://hooks.example");
    assert_eq!(created["channel"]["kind"], "webhook");

    let (status, _, _) = send(
        &app,
        command("POST", "/api/v1/alert-channels")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(json!({
                "id": "chat", "kind": "plugin", "plugin": "chat", "plugin_channel": "#ops"
            })))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, _, listed) = send(
        &app,
        Request::get("/api/v1/alert-channels")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed[0]["etag"], "\"1\"");
    assert!(listed[0].get("secret").is_none());

    let (status, headers, rotated) = send(
        &app,
        command("POST", "/api/v1/alert-channels/ops/rotate")
            .header(header::IF_MATCH, "\"1\"")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{rotated}");
    assert_eq!(headers[header::ETAG], "\"2\"");
    assert_eq!(rotated["secret"], "whsec_bmV3");
    let (status, _, _) = send(
        &app,
        command("POST", "/api/v1/alert-channels/ops/rotate")
            .header(header::IF_MATCH, "\"2\"")
            .header(header::CONTENT_TYPE, "application/json")
            .body(json_body(json!({"url": "https://other.example/x"})))
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _, tested) = send(
        &app,
        command("POST", "/api/v1/alert-channels/ops/test")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        tested,
        json!({"delivered": false, "status": 503, "failure": "the receiver answered 503"})
    );

    let (status, _, notifications) = send(
        &app,
        Request::get("/api/v1/alert-notifications?rule=shop-errors&limit=10")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(notifications[0]["kind"], "resolved");
    assert_eq!(notifications[0]["state"], "queued");
    assert_eq!(notifications[0]["next_attempt_at"], "2027-01-15T08:00:30Z");
    assert_eq!(
        *alerts.calls.lock().unwrap(),
        [
            "create ops https://hooks.example/T0/secret",
            "create chat  Plugin via chat/#ops",
            "rotate ops None 1",
            "rotate ops Some(\"https://other.example/x\") 2",
            "test ops",
            "notifications AlertNotificationQuery { rule: Some(\"shop-errors\"), channel: None, \
             limit: Some(10) }",
        ]
    );
}

#[tokio::test]
async fn without_a_source_alerts_are_unavailable() {
    let app = router(ApiState::new(Arc::new(GatewayService::new(
        Arc::new(FakeGateway),
        Arc::new(IdentityCompiler),
    ))));
    let (status, _, _) = send(
        &app,
        Request::get("/api/v1/alert-rules")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}
