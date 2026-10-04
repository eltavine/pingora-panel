#![forbid(unsafe_code)]

//! Log search, tail and deletion against a stand-in for Loki's HTTP API.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    routing::get,
    Json, Router,
};
use observability_service::{LogsService, Loki};
use panel_contracts::observability::v1::{self as wire, logs_server::Logs};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio_stream::StreamExt;
use tonic::Request;

/// Calls the stand-in received: the endpoint and its query parameters.
type Calls = Arc<Mutex<Vec<(String, HashMap<String, String>)>>>;

#[derive(Clone, Default)]
struct Store {
    calls: Calls,
    /// Range queries fail while set.
    failing: Arc<Mutex<bool>>,
}

fn now_ns() -> i128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i128
}

fn value(time: i128, line: &str, fields: serde_json::Value) -> serde_json::Value {
    serde_json::json!([time.to_string(), line, { "structuredMetadata": fields }])
}

async fn query_range(
    State(store): State<Store>,
    Query(params): Query<HashMap<String, String>>,
) -> (StatusCode, Json<serde_json::Value>) {
    store
        .calls
        .lock()
        .unwrap()
        .push(("query_range".into(), params.clone()));
    if *store.failing.lock().unwrap() {
        return (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": "down" })),
        );
    }
    let start: i128 = params["start"].parse().unwrap();
    let end: i128 = params["end"].parse().unwrap();
    let limit: usize = params["limit"].parse().unwrap();
    let mut records = [
        (now_ns() - 10_000_000_000, "older", "200"),
        (now_ns() - 5_000_000_000, "newer", "502"),
    ];
    if params["direction"] == "backward" {
        records.reverse();
    }
    let values: Vec<_> = records
        .iter()
        .filter(|(time, _, _)| (start..end).contains(time))
        .take(limit)
        .map(|(time, line, status)| {
            value(
                *time,
                line,
                serde_json::json!({
                    "pingora_panel_site_id": "shop",
                    "http_response_status_code": status,
                    "event_name": "pingora_panel.access",
                    "log_file_path": "/var/log/pingora-panel/sites/shop.access.log",
                }),
            )
        })
        .collect();
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "success",
            "data": { "resultType": "streams", "result": [
                { "stream": { "service_name": "pingora-panel-gateway" }, "values": values }
            ] }
        })),
    )
}

async fn delete(
    State(store): State<Store>,
    Query(params): Query<HashMap<String, String>>,
) -> StatusCode {
    store.calls.lock().unwrap().push(("delete".into(), params));
    StatusCode::NO_CONTENT
}

async fn deletions() -> Json<serde_json::Value> {
    Json(serde_json::json!([
        { "request_id": "1", "query": "{service_name=\"pingora-panel-gateway\"} | pingora_panel_site_id = \"shop\"",
          "start_time": 1_700_000_000, "end_time": 1_700_000_600, "created_at": 1_700_000_600_000u64, "status": "received" },
        { "request_id": "2", "query": "{service_name=\"pingora-panel-gateway\"}",
          "start_time": 0, "end_time": 1_600_000_000, "created_at": 1_600_000_000, "status": "processed" },
        { "request_id": "3", "query": "{app=\"other\"}",
          "start_time": 0, "end_time": 1, "created_at": 1, "status": "received" }
    ]))
}

async fn loki() -> (Store, LogsService) {
    let store = Store::default();
    let router = Router::new()
        .route("/loki/api/v1/query_range", get(query_range))
        .route("/loki/api/v1/delete", get(deletions).post(delete))
        .with_state(store.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let service = LogsService::new(Loki::new(&format!("http://{address}")).unwrap());
    (store, service)
}

fn calls(store: &Store, kind: &str) -> Vec<HashMap<String, String>> {
    store
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|(name, _)| name == kind)
        .map(|(_, params)| params.clone())
        .collect()
}

#[tokio::test]
async fn searches_page_newest_first_with_the_query_their_filter_makes() {
    let (store, service) = loki().await;
    let response = service
        .search(Request::new(wire::LogsSearchRequest {
            filter: Some(wire::LogFilter {
                site: "shop".into(),
                status: "5xx".into(),
                ..Default::default()
            }),
            limit: 1,
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(response.error.is_none(), "{:?}", response.error);
    assert_eq!(response.records.len(), 1);
    let record = &response.records[0];
    assert_eq!(record.line, "newer");
    assert_eq!(record.site, "shop");
    assert_eq!(record.status, 502);
    assert_eq!(record.kind, i32::from(wire::LogKind::Access));
    assert!(!record.fields.contains_key("log_file_path"));
    assert_eq!(response.next_until, record.time);

    let call = &calls(&store, "query_range")[0];
    assert_eq!(call["direction"], "backward");
    assert_eq!(call["limit"], "1");
    assert!(call["query"].contains(r#"| pingora_panel_site_id = "shop""#));
    assert!(call["query"].contains("http_response_status_code >= 500"));

    let invalid = service
        .search(Request::new(wire::LogsSearchRequest {
            filter: Some(wire::LogFilter {
                client: "not an address".into(),
                ..Default::default()
            }),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(invalid.error.unwrap().code, "INVALID_ARGUMENT");
    assert_eq!(calls(&store, "query_range").len(), 1);
}

#[tokio::test]
async fn tails_resume_from_their_cursor_and_end_when_the_store_fails() {
    let (store, service) = loki().await;
    let after = now_ns() - 60_000_000_000;
    let mut stream = service
        .tail(Request::new(wire::LogsTailRequest {
            after: Some(prost_types::Timestamp {
                seconds: (after / 1_000_000_000) as i64,
                nanos: 0,
            }),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    let first = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let lines: Vec<_> = first
        .records
        .iter()
        .map(|record| record.line.as_str())
        .collect();
    assert_eq!(lines, ["older", "newer"]);
    assert_eq!(first.cursor, first.records[1].time);
    assert_eq!(calls(&store, "query_range")[0]["direction"], "forward");

    *store.failing.lock().unwrap() = true;
    let ended = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(ended.records.is_empty());
    assert_eq!(ended.cursor, first.cursor);
    assert_eq!(ended.error.unwrap().code, "INTERNAL");
    assert!(tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn deletions_are_asked_for_and_listed() {
    let (store, service) = loki().await;
    let response = service
        .delete(Request::new(wire::LogsDeleteRequest {
            site: "shop".into(),
            since: Some(prost_types::Timestamp {
                seconds: 1_700_000_000,
                nanos: 0,
            }),
            ..Default::default()
        }))
        .await
        .unwrap()
        .into_inner();
    let deletion = response.deletion.unwrap();
    assert_eq!(deletion.site, "shop");
    assert_eq!(deletion.state, i32::from(wire::LogDeletionState::Pending));
    let call = &calls(&store, "delete")[0];
    assert_eq!(
        call["query"],
        r#"{service_name="pingora-panel-gateway"} | pingora_panel_site_id = "shop""#
    );
    assert_eq!(call["start"], "1700000000");
    let end: i64 = call["end"].parse().unwrap();
    assert!(end <= (now_ns() / 1_000_000_000) as i64);

    let listed = service
        .list_deletions(Request::new(wire::LogsListDeletionsRequest::default()))
        .await
        .unwrap()
        .into_inner();
    assert!(listed.error.is_none());
    let summary: Vec<_> = listed
        .deletions
        .iter()
        .map(|deletion| (deletion.site.as_str(), deletion.state))
        .collect();
    assert_eq!(
        summary,
        [
            ("shop", i32::from(wire::LogDeletionState::Pending)),
            ("", i32::from(wire::LogDeletionState::Applied)),
        ]
    );
    assert_eq!(
        listed.deletions[0].requested_at.unwrap().seconds,
        1_700_000_600
    );
}
