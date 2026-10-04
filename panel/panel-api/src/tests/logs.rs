use super::*;
use axum::http::HeaderMap;
use futures_util::{stream, StreamExt};
use panel_application::{
    CommandContext, LogBatch, LogDeletion, LogDeletionState, LogFilter, LogKind, LogPage,
    LogRecord, LogSearch, LogTail, LogsPort, RequestScope,
};
use panel_domain::{RouteId, SiteId};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    future::IntoFuture,
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio_tungstenite::tungstenite::{self, protocol::frame::coding::CloseCode};

fn at(seconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_800_000_000 + seconds)
}

fn record(seconds: u64, line: &str) -> LogRecord {
    LogRecord {
        time: at(seconds) + Duration::from_nanos(5),
        kind: LogKind::Access,
        line: line.to_owned(),
        site: Some("shop".into()),
        route: Some("checkout".into()),
        status: Some(502),
        method: Some("GET".into()),
        path: Some("/cart".into()),
        client: Some("192.0.2.1".into()),
        request_id: Some("r1".into()),
        fields: BTreeMap::from([("http_response_status_code".into(), "502".into())]),
    }
}

/// Who asked to delete which site's records from when.
type Delete = (String, Option<SiteId>, Option<SystemTime>);

#[derive(Default)]
struct Logs {
    searches: Mutex<Vec<LogSearch>>,
    tails: Mutex<Vec<(LogFilter, Option<SystemTime>)>>,
    deletes: Mutex<Vec<Delete>>,
}

#[async_trait]
impl LogsPort for Logs {
    async fn search(&self, _scope: RequestScope, search: LogSearch) -> Result<LogPage> {
        self.searches.lock().unwrap().push(search.clone());
        Ok(match search.until {
            Some(until) if until == at(2) => LogPage {
                records: vec![record(1, "third")],
                next_until: None,
            },
            _ => LogPage {
                records: vec![record(3, "first\n"), record(2, "second")],
                next_until: Some(at(2)),
            },
        })
    }

    async fn tail(
        &self,
        _scope: RequestScope,
        filter: LogFilter,
        after: Option<SystemTime>,
    ) -> Result<LogTail> {
        self.tails.lock().unwrap().push((filter, after));
        Ok(Box::pin(stream::iter([
            Ok(LogBatch {
                records: vec![record(4, "live")],
                cursor: at(4),
            }),
            Err(PanelError::resource_exhausted("the tail fell behind")),
        ])))
    }

    async fn delete(
        &self,
        context: CommandContext,
        site: Option<SiteId>,
        since: Option<SystemTime>,
    ) -> Result<LogDeletion> {
        self.deletes
            .lock()
            .unwrap()
            .push((context.actor().to_owned(), site.clone(), since));
        Ok(LogDeletion {
            site: site.map(|site| site.as_str().to_owned()),
            since: since.unwrap_or(UNIX_EPOCH),
            until: at(9),
            requested_at: at(9),
            state: LogDeletionState::Pending,
        })
    }

    async fn deletions(&self, _scope: RequestScope) -> Result<Vec<LogDeletion>> {
        Ok(vec![LogDeletion {
            site: None,
            since: UNIX_EPOCH,
            until: at(5),
            requested_at: at(5),
            state: LogDeletionState::Applied,
        }])
    }
}

fn logs_app(logs: Arc<Logs>) -> axum::Router {
    router(
        ApiState::new(Arc::new(GatewayService::new(
            Arc::new(FakeGateway),
            Arc::new(IdentityCompiler),
        )))
        .with_logs(logs),
    )
}

async fn send(app: &axum::Router, request: Request<Body>) -> (StatusCode, HeaderMap, Vec<u8>) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, headers, bytes.to_vec())
}

async fn get(app: &axum::Router, uri: &str) -> (StatusCode, Value) {
    let (status, _, body) = send(app, Request::get(uri).body(Body::empty()).unwrap()).await;
    (status, serde_json::from_slice(&body).unwrap())
}

#[tokio::test]
async fn searches_pass_their_filter_and_window() {
    let logs = Arc::new(Logs::default());
    let app = logs_app(Arc::clone(&logs));

    let (status, page) = get(
        &app,
        "/api/v1/logs?kind=access&site=shop&route=checkout&status=5xx&client=10.0.0.0/8\
         &path=/cart&request_id=r1&text=boom&since=2027-01-15T08:00:00Z\
         &until=2027-01-15T09:00:00.5%2B01:00&limit=50",
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(
        page["records"][0],
        json!({
            "time": "2027-01-15T08:00:03.000000005Z",
            "kind": "access",
            "line": "first\n",
            "site": "shop",
            "route": "checkout",
            "status": 502,
            "method": "GET",
            "path": "/cart",
            "client": "192.0.2.1",
            "request_id": "r1",
            "fields": {"http_response_status_code": "502"}
        })
    );
    assert_eq!(page["next_until"], "2027-01-15T08:00:02Z");
    assert_eq!(
        logs.searches.lock().unwrap()[0],
        LogSearch {
            filter: LogFilter {
                kind: Some(LogKind::Access),
                site: Some(SiteId::new("shop").unwrap()),
                route: Some(RouteId::new("checkout").unwrap()),
                status: Some("5xx".into()),
                client: Some("10.0.0.0/8".into()),
                path_prefix: Some("/cart".into()),
                request_id: Some("r1".into()),
                text: Some("boom".into()),
            },
            since: Some(at(0)),
            until: Some(at(0) + Duration::from_millis(500)),
            limit: Some(50),
        }
    );
}

#[tokio::test]
async fn odd_times_and_names_are_refused() {
    let app = logs_app(Arc::new(Logs::default()));
    for uri in [
        "/api/v1/logs?since=yesterday",
        "/api/v1/logs?site=not%20an%20id",
        "/api/v1/logs?kind=debug",
    ] {
        let (status, _, _) = send(&app, Request::get(uri).body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
    }
}

#[tokio::test]
async fn downloads_are_every_page_as_lines() {
    let logs = Arc::new(Logs::default());
    let app = logs_app(Arc::clone(&logs));

    let (status, headers, body) = send(
        &app,
        Request::get("/api/v1/logs/download?site=shop&limit=2")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "text/plain; charset=utf-8");
    assert!(headers[header::CONTENT_DISPOSITION]
        .to_str()
        .unwrap()
        .starts_with("attachment; filename=\"gateway-"));
    assert_eq!(String::from_utf8(body).unwrap(), "first\nsecond\nthird\n");
    let searches = logs.searches.lock().unwrap();
    assert_eq!(searches.len(), 2);
    assert!(searches.iter().all(|search| search.limit == Some(500)));
    assert_eq!(searches[1].until, Some(at(2)));
}

#[tokio::test]
async fn tails_are_relayed_over_a_websocket_until_they_end() {
    let logs = Arc::new(Logs::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(IntoFuture::into_future(axum::serve(
        listener,
        logs_app(Arc::clone(&logs)),
    )));

    let (mut socket, _) = tokio_tungstenite::connect_async(format!(
        "ws://{address}/api/v1/logs/tail?kind=error&after=2027-01-15T08:00:00Z"
    ))
    .await
    .unwrap();
    let text = |message: tungstenite::Message| -> Value {
        serde_json::from_str(message.to_text().unwrap()).unwrap()
    };
    let batch = text(socket.next().await.unwrap().unwrap());
    assert_eq!(batch["records"][0]["line"], "live");
    assert_eq!(batch["cursor"], "2027-01-15T08:00:04Z");
    assert_eq!(batch["error"], Value::Null);
    let ended = text(socket.next().await.unwrap().unwrap());
    assert_eq!(ended["records"], json!([]));
    assert_eq!(ended["cursor"], "2027-01-15T08:00:04Z");
    assert_eq!(ended["error"]["code"], "RESOURCE_EXHAUSTED");
    match socket.next().await.unwrap().unwrap() {
        tungstenite::Message::Close(Some(frame)) => assert_eq!(frame.code, CloseCode::Again),
        other => panic!("expected a close frame, got {other:?}"),
    }
    let _ = socket.close(None).await;
    assert_eq!(
        logs.tails.lock().unwrap()[0],
        (
            LogFilter {
                kind: Some(LogKind::Error),
                ..LogFilter::default()
            },
            Some(at(0))
        )
    );
}

#[tokio::test]
async fn deletions_are_accepted_and_listed() {
    let logs = Arc::new(Logs::default());
    let app = logs_app(Arc::clone(&logs));

    let (status, _, body) = send(
        &app,
        Request::post("/api/v1/logs/deletions")
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-actor", "root")
            .header("x-deadline", "2099-01-01T00:00:00Z")
            .header("idempotency-key", "delete-1")
            .body(Body::from(
                json!({"site": "shop", "since": "2027-01-15T08:00:00Z"}).to_string(),
            ))
            .unwrap(),
    )
    .await;
    let deletion: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, StatusCode::ACCEPTED, "{deletion}");
    assert_eq!(
        deletion,
        json!({
            "site": "shop",
            "since": "2027-01-15T08:00:00Z",
            "until": "2027-01-15T08:00:09Z",
            "requested_at": "2027-01-15T08:00:09Z",
            "state": "pending"
        })
    );
    assert_eq!(
        logs.deletes.lock().unwrap()[0],
        (
            "root".to_owned(),
            Some(SiteId::new("shop").unwrap()),
            Some(at(0))
        )
    );

    let (status, listed) = get(&app, "/api/v1/logs/deletions").await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(listed["deletions"][0]["site"], Value::Null);
    assert_eq!(listed["deletions"][0]["since"], "1970-01-01T00:00:00Z");
    assert_eq!(listed["deletions"][0]["state"], "applied");
}

#[tokio::test]
async fn without_a_source_logs_are_unavailable() {
    let app = router(ApiState::new(Arc::new(GatewayService::new(
        Arc::new(FakeGateway),
        Arc::new(IdentityCompiler),
    ))));
    let (status, _) = get(&app, "/api/v1/logs").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
}
