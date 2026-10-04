#![forbid(unsafe_code)]

//! Alert rules evaluated against a stand-in Prometheus, notifying a
//! stand-in webhook receiver that verifies signatures as Standard Webhooks
//! receivers do.

use axum::{
    extract::{RawQuery, State as Shared},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use chrono::{TimeDelta, Utc};
use observability_service::{
    prometheus, AlertChannels, AlertRules, Cause, ChannelKind, Comparison, Evaluator, Measure,
    Notices, Notifier, RuleSpec, Severity, State, MIGRATIONS,
};
use panel_domain::SiteId;
use panel_errors::ErrorCode;
use panel_events::{Principal, RequestId, RequestScope};
use panel_platform::ServiceName;
use panel_secrets::{EnvelopeVault, SecretVault};
use panel_sqlite::{testing::TestDatabase, EventLog, ServiceDatabase};
use serde_json::{json, Value};
use standardwebhooks::Webhook;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

async fn database() -> (TestDatabase, ServiceDatabase) {
    let database = TestDatabase::migrated(MIGRATIONS).await;
    let service = database.database().clone();
    (database, service)
}

async fn serve(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    format!("http://{address}")
}

/// What the stand-in Prometheus reads for every query; no data when unset.
type Reading = Arc<Mutex<Option<f64>>>;

async fn prometheus_stand_in(reading: Reading) -> String {
    let query = |Shared(reading): Shared<Reading>| async move {
        let result: Vec<Value> = reading
            .lock()
            .unwrap()
            .map(|value| json!({"metric": {}, "value": [1_700_000_000, value.to_string()]}))
            .into_iter()
            .collect();
        Json(json!({"status": "success", "data": {"resultType": "vector", "result": result}}))
    };
    serve(
        Router::new()
            .route("/api/v1/query", get(query).post(query))
            .with_state(reading),
    )
    .await
}

/// What a receiver was sent: headers, body and query string.
type Received = Vec<(HeaderMap, String, Option<String>)>;

#[derive(Clone, Default)]
struct Receiver {
    received: Arc<Mutex<Received>>,
    /// Statuses to answer with, in turn; 200 once they run out.
    answers: Arc<Mutex<VecDeque<u16>>>,
}

async fn receiver_stand_in(receiver: Receiver) -> String {
    let hook = |Shared(receiver): Shared<Receiver>,
                RawQuery(query): RawQuery,
                headers: HeaderMap,
                body: String| async move {
        receiver
            .received
            .lock()
            .unwrap()
            .push((headers, body, query));
        let status = receiver.answers.lock().unwrap().pop_front().unwrap_or(200);
        StatusCode::from_u16(status).unwrap()
    };
    serve(
        Router::new()
            .route("/hook", post(hook))
            .with_state(receiver),
    )
    .await
}

struct Alerts {
    rules: AlertRules,
    channels: AlertChannels,
    notifier: Notifier,
    evaluator: Evaluator,
    service: ServiceDatabase,
}

fn alerts(service: ServiceDatabase, prometheus_url: &str) -> Alerts {
    let events = EventLog::new(&service, ServiceName::new("observability-service").unwrap());
    let vault: Arc<dyn SecretVault> =
        Arc::new(EnvelopeVault::from_keys(&EnvelopeVault::generate_key().unwrap()).unwrap());
    let notices = Arc::new(Notices::new(Some("https://panel.example".into())));
    let rules = AlertRules::new(&service, events.clone(), Arc::clone(&notices));
    let channels = AlertChannels::new(&service, events, Some(vault));
    let notifier = Notifier::new(&service, channels.clone(), notices).unwrap();
    let evaluator = Evaluator::new(
        rules.clone(),
        prometheus(prometheus_url).unwrap(),
        "observability-service",
    )
    .unwrap();
    Alerts {
        rules,
        channels,
        notifier,
        evaluator,
        service,
    }
}

fn errors_rule(pending: u64) -> RuleSpec {
    RuleSpec {
        name: "Shop errors".into(),
        description: "Too many requests to the shop fail.".into(),
        measure: Measure::ServerErrorRatio,
        comparison: Comparison::Above,
        threshold: 0.1,
        pending_for: Duration::from_secs(pending),
        site: Some(SiteId::new("shop").unwrap()),
        route: None,
        upstream: None,
        severity: Severity::Critical,
        enabled: true,
        channels: vec!["ops".into()],
    }
}

async fn events(service: &ServiceDatabase) -> Vec<String> {
    sqlx::query_scalar("SELECT event_type FROM outbox ORDER BY position")
        .fetch_all(service.pool())
        .await
        .unwrap()
}

/// The request and the operator that change rules and channels.
fn caller() -> (RequestScope, Principal) {
    (
        RequestScope::new(RequestId::new("request-1").unwrap()),
        EventLog::user("ops"),
    )
}

#[tokio::test]
async fn firing_and_resolving_alerts_notify_signed_webhooks() {
    let (_database, service) = database().await;
    let reading = Reading::default();
    let alerts = alerts(service, &prometheus_stand_in(Arc::clone(&reading)).await);
    let receiver = Receiver::default();
    let base = receiver_stand_in(receiver.clone()).await;
    let (scope, principal) = caller();
    let cause = Cause {
        scope: &scope,
        principal: &principal,
    };

    let (channel, secret) = alerts
        .channels
        .create(
            cause,
            "ops",
            ChannelKind::Webhook,
            &format!("{base}/hook?token=abc"),
        )
        .await
        .unwrap();
    assert_eq!(channel.target, base, "only the origin is shown");
    assert!(secret.starts_with("whsec_"));
    alerts
        .rules
        .put(cause, "shop-errors", errors_rule(0), 0)
        .await
        .unwrap();

    *reading.lock().unwrap() = Some(0.5);
    alerts.evaluator.evaluate(Utc::now()).await.unwrap();
    let firing = alerts.rules.get("shop-errors").await.unwrap();
    assert_eq!(firing.state, State::Firing);
    assert_eq!(firing.value, Some(0.5));
    assert!(alerts.notifier.deliver_next().await.unwrap());
    assert!(!alerts.notifier.deliver_next().await.unwrap());

    let (headers, body, query) = receiver.received.lock().unwrap()[0].clone();
    assert_eq!(query.as_deref(), Some("token=abc"));
    Webhook::new(&secret)
        .unwrap()
        .verify(body.as_bytes(), &headers)
        .expect("the signature verifies");
    let payload: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(payload["version"], "4");
    assert_eq!(payload["status"], "firing");
    assert_eq!(payload["receiver"], "ops");
    assert_eq!(payload["externalURL"], "https://panel.example/");
    let alert = &payload["alerts"][0];
    assert_eq!(alert["labels"]["alertname"], "Shop errors");
    assert_eq!(alert["labels"]["site"], "shop");
    assert_eq!(alert["labels"]["severity"], "critical");
    assert_eq!(alert["annotations"]["value"], "0.5");
    assert_eq!(alert["endsAt"], "0001-01-01T00:00:00Z");
    assert_eq!(
        alert["generatorURL"],
        "https://panel.example/alerts?rule=shop-errors"
    );
    assert_eq!(
        headers["webhook-id"].to_str().unwrap(),
        alerts.notifier.list(None, None, None).await.unwrap()[0]
            .id
            .to_string()
    );

    // A quiet evaluation keeps nothing firing and resolves the alert.
    *reading.lock().unwrap() = Some(0.01);
    alerts.evaluator.evaluate(Utc::now()).await.unwrap();
    assert_eq!(
        alerts.rules.get("shop-errors").await.unwrap().state,
        State::Inactive
    );
    assert!(alerts.notifier.deliver_next().await.unwrap());
    let resolved: Value = serde_json::from_str(&receiver.received.lock().unwrap()[1].1).unwrap();
    assert_eq!(resolved["status"], "resolved");
    assert_ne!(resolved["alerts"][0]["endsAt"], "0001-01-01T00:00:00Z");
    let delivered = alerts
        .notifier
        .list(Some("shop-errors"), None, None)
        .await
        .unwrap();
    assert_eq!(delivered.len(), 2);
    assert!(delivered
        .iter()
        .all(|notification| notification.state == "delivered"));
    assert_eq!(
        (delivered[0].kind.as_str(), delivered[1].kind.as_str()),
        ("resolved", "firing")
    );

    let recorded = events(&alerts.service).await;
    for expected in [
        "observability.alert_channel.created",
        "observability.alert_rule.created",
        "observability.alert.fired",
        "observability.alert.resolved",
    ] {
        assert!(
            recorded
                .iter()
                .any(|event| event.contains(&format!(".{expected}."))),
            "{expected} in {recorded:?}"
        );
    }
}

#[tokio::test]
async fn alerts_wait_for_their_pending_period_and_unreadable_rules_keep_their_state() {
    let (_database, service) = database().await;
    let reading = Reading::default();
    let alerts = alerts(service, &prometheus_stand_in(Arc::clone(&reading)).await);
    let base = receiver_stand_in(Receiver::default()).await;
    let (scope, principal) = caller();
    let cause = Cause {
        scope: &scope,
        principal: &principal,
    };
    alerts
        .channels
        .create(cause, "ops", ChannelKind::Webhook, &format!("{base}/hook"))
        .await
        .unwrap();
    alerts
        .rules
        .put(cause, "shop-errors", errors_rule(60), 0)
        .await
        .unwrap();

    *reading.lock().unwrap() = Some(0.5);
    let start = Utc::now();
    alerts.evaluator.evaluate(start).await.unwrap();
    let pending = alerts.rules.get("shop-errors").await.unwrap();
    assert_eq!(pending.state, State::Pending);
    assert!(alerts
        .notifier
        .list(None, None, None)
        .await
        .unwrap()
        .is_empty());
    alerts
        .evaluator
        .evaluate(start + TimeDelta::seconds(61))
        .await
        .unwrap();
    assert_eq!(
        alerts.rules.get("shop-errors").await.unwrap().state,
        State::Firing
    );

    // Without data, a ratio does not hold: the alert resolves.
    *reading.lock().unwrap() = None;
    alerts
        .evaluator
        .evaluate(start + TimeDelta::seconds(90))
        .await
        .unwrap();
    assert_eq!(
        alerts.rules.get("shop-errors").await.unwrap().state,
        State::Inactive
    );

    // An unreachable Prometheus neither fires nor resolves, and says why.
    let unreachable = observability_service::Evaluator::new(
        alerts.rules.clone(),
        prometheus("http://127.0.0.1:9").unwrap(),
        "observability-service",
    )
    .unwrap();
    unreachable
        .evaluate(start + TimeDelta::seconds(120))
        .await
        .unwrap();
    let kept = alerts.rules.get("shop-errors").await.unwrap();
    assert_eq!(kept.state, State::Inactive);
    assert!(!kept.evaluation_error.is_empty());
}

#[tokio::test]
async fn failed_notifications_are_retried_and_refused_ones_abandoned() {
    let (_database, service) = database().await;
    let reading = Reading::default();
    let alerts = alerts(service, &prometheus_stand_in(Arc::clone(&reading)).await);
    let receiver = Receiver::default();
    receiver.answers.lock().unwrap().extend([503, 410]);
    let base = receiver_stand_in(receiver.clone()).await;
    let (scope, principal) = caller();
    let cause = Cause {
        scope: &scope,
        principal: &principal,
    };
    alerts
        .channels
        .create(cause, "ops", ChannelKind::Webhook, &format!("{base}/hook"))
        .await
        .unwrap();
    alerts
        .rules
        .put(cause, "shop-errors", errors_rule(0), 0)
        .await
        .unwrap();
    *reading.lock().unwrap() = Some(0.5);
    alerts.evaluator.evaluate(Utc::now()).await.unwrap();

    assert!(alerts.notifier.deliver_next().await.unwrap());
    let retried = &alerts.notifier.list(None, None, None).await.unwrap()[0];
    assert_eq!((retried.state.as_str(), retried.attempts), ("queued", 1));
    assert_eq!(retried.last_failure, "the receiver answered 503");
    assert!(retried.next_attempt_at.unwrap() > Utc::now());
    assert!(
        !alerts.notifier.deliver_next().await.unwrap(),
        "not due yet"
    );

    sqlx::query("UPDATE alert_notifications SET next_attempt_at = ?1")
        .bind(Utc::now())
        .execute(alerts.service.pool())
        .await
        .unwrap();
    assert!(alerts.notifier.deliver_next().await.unwrap());
    let abandoned = &alerts.notifier.list(None, None, None).await.unwrap()[0];
    assert_eq!(
        (abandoned.state.as_str(), abandoned.attempts),
        ("abandoned", 2)
    );
    assert_eq!(abandoned.next_attempt_at, None);

    // A test reports how the receiver answered, without queueing anything.
    let tested = alerts.notifier.test("ops").await.unwrap();
    assert!(tested.delivered);
    assert_eq!(tested.status, Some(200));
    let test: Value = serde_json::from_str(&receiver.received.lock().unwrap()[2].1).unwrap();
    assert_eq!(test["alerts"][0]["labels"]["test"], "true");
    assert_eq!(
        alerts.notifier.list(None, None, None).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn rules_and_channels_refuse_what_they_cannot_keep() {
    let (_database, service) = database().await;
    let alerts = alerts(service, "http://127.0.0.1:9");
    let (scope, principal) = caller();
    let cause = Cause {
        scope: &scope,
        principal: &principal,
    };

    let email = alerts
        .channels
        .create(cause, "mail", ChannelKind::Email, "")
        .await
        .unwrap_err();
    assert_eq!(email.code.as_str(), ErrorCode::UNSUPPORTED_CAPABILITY);
    let unknown = alerts
        .rules
        .put(cause, "shop-errors", errors_rule(0), 0)
        .await
        .unwrap_err();
    assert_eq!(unknown.code.as_str(), ErrorCode::INVALID_ARGUMENT);

    let (_, first) = alerts
        .channels
        .create(
            cause,
            "ops",
            ChannelKind::Webhook,
            "https://hooks.example/a",
        )
        .await
        .unwrap();
    let (rotated, second) = alerts
        .channels
        .rotate(cause, "ops", Some("https://other.example/b"), 1)
        .await
        .unwrap();
    assert_ne!(*first, *second);
    assert_eq!(
        (rotated.target.as_str(), rotated.version),
        ("https://other.example", 2)
    );
    let stale = alerts
        .channels
        .rotate(cause, "ops", None, 1)
        .await
        .unwrap_err();
    assert_eq!(stale.code.as_str(), ErrorCode::PRECONDITION_FAILED);

    let created = alerts
        .rules
        .put(cause, "shop-errors", errors_rule(0), 0)
        .await
        .unwrap();
    assert_eq!(created.version, 1);
    let again = alerts
        .rules
        .put(cause, "shop-errors", errors_rule(0), 0)
        .await
        .unwrap_err();
    assert_eq!(again.code.as_str(), ErrorCode::CONFLICT);
    let mut quieter = errors_rule(120);
    quieter.threshold = 0.2;
    let updated = alerts
        .rules
        .put(cause, "shop-errors", quieter, 1)
        .await
        .unwrap();
    assert_eq!((updated.version, updated.spec.threshold), (2, 0.2));
    let in_use = alerts.channels.delete(cause, "ops", 0).await.unwrap_err();
    assert_eq!(in_use.code.as_str(), ErrorCode::CONFLICT);

    alerts.rules.delete(cause, "shop-errors", 2).await.unwrap();
    alerts.channels.delete(cause, "ops", 0).await.unwrap();
    assert!(alerts.rules.list().await.unwrap().is_empty());
    assert!(alerts.channels.list().await.unwrap().is_empty());

    let recorded = events(&alerts.service).await;
    for expected in [
        "observability.alert_channel.refused",
        "observability.alert_rule.refused",
        "observability.alert_channel.rotated",
        "observability.alert_rule.updated",
        "observability.alert_rule.deleted",
        "observability.alert_channel.deleted",
    ] {
        assert!(
            recorded
                .iter()
                .any(|event| event.contains(&format!(".{expected}."))),
            "{expected} in {recorded:?}"
        );
    }
}
