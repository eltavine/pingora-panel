//! The gateway's access and error logs (ADR 0026): searched a page at a
//! time, followed over a WebSocket, downloaded as a log file and deleted.

use crate::{
    error::ApiError,
    request_context::{command_context, request_scope, MutationHeaders, QueryHeaders},
    ApiState,
};
use axum::{
    body::{Body, Bytes},
    extract::{
        ws::{CloseFrame, Message, Utf8Bytes, WebSocket, WebSocketUpgrade},
        Query, State,
    },
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, SecondsFormat, Utc};
use futures_util::{stream, Stream, StreamExt};
use panel_application::{
    LogDeletion, LogDeletionState, LogFilter, LogKind, LogPage, LogRecord, LogSearch, LogTail,
    LogsPort, RequestScope,
};
use panel_domain::{RouteId, SiteId};
use panel_errors::{ErrorCode, PanelError};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc, time::Duration, time::SystemTime};
use utoipa::{IntoParams, ToSchema};

/// The most records a download holds.
const DOWNLOAD_LIMIT: usize = 100_000;
/// Records a download reads at a time.
const PAGE: u32 = 500;
/// How often an idle tail is pinged.
const PING: Duration = Duration::from_secs(30);

fn port<U>(state: &ApiState<U>) -> Result<Arc<dyn LogsPort>, ApiError> {
    state.logs.clone().ok_or_else(|| {
        ApiError::new(PanelError::unavailable(
            "the gateway's logs are not available here",
        ))
    })
}

fn rfc3339(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

fn parse_time(name: &str, value: Option<String>) -> Result<Option<SystemTime>, ApiError> {
    value
        .filter(|value| !value.is_empty())
        .map(|value| {
            DateTime::parse_from_rfc3339(&value)
                .map(SystemTime::from)
                .map_err(|_| {
                    ApiError::new(PanelError::invalid_argument(format!(
                        "{name} is not an RFC 3339 time"
                    )))
                })
        })
        .transpose()
}

fn identifier<T, E>(
    name: &str,
    value: Option<String>,
    parse: impl FnOnce(String) -> Result<T, E>,
) -> Result<Option<T>, ApiError> {
    value
        .filter(|value| !value.is_empty())
        .map(parse)
        .transpose()
        .map_err(|_| {
            ApiError::new(PanelError::invalid_argument(format!(
                "{name} is not an identifier"
            )))
        })
}

/// Whether a record tells what a request did or what went wrong.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LogKindName {
    Access,
    Error,
}

/// One record, with the fields the console shows on their own.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct LogRecordItem {
    /// RFC 3339, to the nanosecond the record carries.
    pub time: String,
    pub kind: LogKindName,
    /// The line as the gateway wrote it.
    pub line: String,
    pub site: Option<String>,
    pub route: Option<String>,
    pub status: Option<u16>,
    pub method: Option<String>,
    pub path: Option<String>,
    pub client: Option<String>,
    pub request_id: Option<String>,
    /// Every field the log store keeps, by its name there.
    pub fields: BTreeMap<String, String>,
}

impl From<LogRecord> for LogRecordItem {
    fn from(value: LogRecord) -> Self {
        Self {
            time: rfc3339(value.time),
            kind: match value.kind {
                LogKind::Error => LogKindName::Error,
                _ => LogKindName::Access,
            },
            line: value.line,
            site: value.site,
            route: value.route,
            status: value.status,
            method: value.method,
            path: value.path,
            client: value.client,
            request_id: value.request_id,
            fields: value.fields,
        }
    }
}

/// Records newest first.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct LogPageResponse {
    pub records: Vec<LogRecordItem>,
    /// Pass as `until` to read the next page; absent after the last one.
    pub next_until: Option<String>,
}

/// What a tail sends: records oldest first and where to resume, or why it
/// ends.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct LogTailMessage {
    pub records: Vec<LogRecordItem>,
    /// Pass as `after` to resume after these records.
    pub cursor: Option<String>,
    pub error: Option<LogTailError>,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct LogTailError {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum LogDeletionStateName {
    /// Asked for; the records remain until the log store applies it.
    Pending,
    Applied,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct LogDeletionItem {
    /// Absent for every site.
    pub site: Option<String>,
    pub since: String,
    pub until: String,
    pub requested_at: String,
    pub state: LogDeletionStateName,
}

impl From<LogDeletion> for LogDeletionItem {
    fn from(value: LogDeletion) -> Self {
        Self {
            site: value.site,
            since: rfc3339(value.since),
            until: rfc3339(value.until),
            requested_at: rfc3339(value.requested_at),
            state: match value.state {
                LogDeletionState::Applied => LogDeletionStateName::Applied,
                _ => LogDeletionStateName::Pending,
            },
        }
    }
}

/// Deletions newest first.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct LogDeletionList {
    pub deletions: Vec<LogDeletionItem>,
}

/// Which records to delete.
#[derive(Clone, Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LogDeletionRequest {
    /// The site whose records go; every site's when absent.
    #[serde(default)]
    pub site: Option<String>,
    /// RFC 3339; every older record goes too when absent.
    #[serde(default)]
    pub since: Option<String>,
}

#[derive(Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct LogParams {
    /// `access` or `error`; both when absent.
    kind: Option<LogKindName>,
    site: Option<String>,
    route: Option<String>,
    /// A status code such as `502`, or a class such as `5xx`.
    status: Option<String>,
    /// A client address, or a CIDR block of them.
    client: Option<String>,
    /// Requests whose path starts with this.
    path: Option<String>,
    request_id: Option<String>,
    /// Text the record contains, ignoring case.
    text: Option<String>,
    /// RFC 3339: records at or after this time; an hour before `until` by
    /// default. Searches and downloads.
    since: Option<String>,
    /// RFC 3339: records before this time; now by default. Searches and
    /// downloads.
    until: Option<String>,
    /// Records per page, at most 500; 100 by default. Searches.
    limit: Option<u32>,
    /// RFC 3339: records after this time; now by default. Tails.
    after: Option<String>,
}

impl LogParams {
    fn filter(&mut self) -> Result<LogFilter, ApiError> {
        let text = |value: &mut Option<String>| value.take().filter(|value| !value.is_empty());
        Ok(LogFilter {
            kind: self.kind.map(|kind| match kind {
                LogKindName::Access => LogKind::Access,
                LogKindName::Error => LogKind::Error,
            }),
            site: identifier("site", self.site.take(), SiteId::new)?,
            route: identifier("route", self.route.take(), RouteId::new)?,
            status: text(&mut self.status),
            client: text(&mut self.client),
            path_prefix: text(&mut self.path),
            request_id: text(&mut self.request_id),
            text: text(&mut self.text),
        })
    }

    fn search(mut self) -> Result<LogSearch, ApiError> {
        Ok(LogSearch {
            filter: self.filter()?,
            since: parse_time("since", self.since.take())?,
            until: parse_time("until", self.until.take())?,
            limit: self.limit,
        })
    }
}

/// Records that match, newest first, one page at a time.
#[utoipa::path(get, path = "/api/v1/logs", params(QueryHeaders, LogParams),
    responses((status = 200, body = LogPageResponse)), tag = "logs")]
pub(crate) async fn search_logs<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Query(params): Query<LogParams>,
) -> Result<Json<LogPageResponse>, ApiError> {
    let scope = request_scope(&headers)?;
    let page = port(&state)?.search(scope, params.search()?).await?;
    Ok(Json(LogPageResponse {
        records: page.records.into_iter().map(Into::into).collect(),
        next_until: page.next_until.map(rfc3339),
    }))
}

/// The lines of up to 100,000 matching records, newest first, as a log
/// file.
#[utoipa::path(get, path = "/api/v1/logs/download", params(QueryHeaders, LogParams),
    responses((status = 200, content_type = "text/plain", body = String)), tag = "logs")]
pub(crate) async fn download_logs<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Query(params): Query<LogParams>,
) -> Result<Response, ApiError> {
    let scope = request_scope(&headers)?;
    let mut search = params.search()?;
    search.limit = Some(PAGE);
    let port = port(&state)?;
    // The first page is read before answering, so a bad filter is a 400.
    let first = port.search(scope.clone(), search.clone()).await?;
    let lines = lines(port, scope, Download::Write(first, search, 0));
    let name = format!(
        "attachment; filename=\"gateway-{}.log\"",
        Utc::now().format("%Y%m%dT%H%M%SZ")
    );
    Ok((
        [
            (
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            ),
            (
                header::CONTENT_DISPOSITION,
                HeaderValue::from_str(&name).expect("the file name is ASCII"),
            ),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        Body::from_stream(lines),
    )
        .into_response())
}

/// Where a download is: a page to write, the next page to read, or done;
/// each with the search and the records written so far.
enum Download {
    Write(LogPage, LogSearch, usize),
    Read(LogSearch, usize),
    Done,
}

/// The lines of each page in turn, read as the body is, until the limit or
/// the last page.
fn lines(
    port: Arc<dyn LogsPort>,
    scope: RequestScope,
    start: Download,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send {
    stream::unfold(start, move |download| {
        let port = Arc::clone(&port);
        let scope = scope.clone();
        async move {
            let (page, mut search, mut written) = match download {
                Download::Done => return None,
                Download::Write(page, search, written) => (page, search, written),
                Download::Read(search, written) => match port.search(scope, search.clone()).await {
                    Ok(page) => (page, search, written),
                    Err(error) => {
                        let error = std::io::Error::other(error.message);
                        return Some((Err(error), Download::Done));
                    }
                },
            };
            let mut chunk = String::new();
            for record in page.records.into_iter().take(DOWNLOAD_LIMIT - written) {
                chunk.push_str(record.line.trim_end_matches('\n'));
                chunk.push('\n');
                written += 1;
            }
            let next = match page.next_until.filter(|_| written < DOWNLOAD_LIMIT) {
                Some(until) => {
                    search.until = Some(until);
                    Download::Read(search, written)
                }
                None => Download::Done,
            };
            Some((Ok(Bytes::from(chunk)), next))
        }
    })
}

fn tail_message(records: Vec<LogRecord>, cursor: Option<SystemTime>) -> LogTailMessage {
    LogTailMessage {
        records: records.into_iter().map(Into::into).collect(),
        cursor: cursor.map(rfc3339),
        error: None,
    }
}

async fn send(socket: &mut WebSocket, message: &LogTailMessage) -> bool {
    let text = serde_json::to_string(message).expect("tail messages serialize");
    socket
        .send(Message::Text(Utf8Bytes::from(text)))
        .await
        .is_ok()
}

/// Relays `tail` to `socket` until either side ends it.
async fn relay(mut socket: WebSocket, mut tail: LogTail) {
    let mut ping = tokio::time::interval(PING);
    ping.tick().await;
    let mut cursor = None;
    loop {
        tokio::select! {
            batch = tail.next() => {
                match batch {
                    Some(Ok(batch)) => {
                        cursor = Some(batch.cursor);
                        if !send(&mut socket, &tail_message(batch.records, cursor)).await {
                            return;
                        }
                    }
                    Some(Err(error)) => {
                        let code = match error.code.as_str() {
                            ErrorCode::INVALID_ARGUMENT => 1008,
                            ErrorCode::UNAVAILABLE | ErrorCode::RESOURCE_EXHAUSTED => 1013,
                            _ => 1011,
                        };
                        let mut message = tail_message(Vec::new(), cursor);
                        message.error = Some(LogTailError {
                            code: error.code.as_str().to_owned(),
                            message: error.message,
                        });
                        let _ = send(&mut socket, &message).await;
                        let _ = socket
                            .send(Message::Close(Some(CloseFrame {
                                code,
                                reason: Utf8Bytes::from_static("the tail ended"),
                            })))
                            .await;
                        return;
                    }
                    None => {
                        let _ = socket.send(Message::Close(None)).await;
                        return;
                    }
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                    Some(Ok(_)) => {}
                }
            }
            _ = ping.tick() => {
                if socket.send(Message::Ping(Bytes::new())).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// Follows matching records as they arrive, over a WebSocket. Each text
/// message is a `LogTailMessage`; one with an error is the last.
#[utoipa::path(get, path = "/api/v1/logs/tail", params(QueryHeaders, LogParams),
    responses((status = 101, description = "Switching to the WebSocket protocol")), tag = "logs")]
pub(crate) async fn tail_logs<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Query(mut params): Query<LogParams>,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let scope = request_scope(&headers)?;
    let filter = params.filter()?;
    let after = parse_time("after", params.after.take())?;
    let tail = port(&state)?.tail(scope, filter, after).await?;
    Ok(upgrade.on_upgrade(move |socket| relay(socket, tail)))
}

/// Asks the log store to delete records of one site or every site; it
/// applies the deletion after its cancel period.
#[utoipa::path(post, path = "/api/v1/logs/deletions", request_body = LogDeletionRequest,
    params(MutationHeaders), responses((status = 202, body = LogDeletionItem)), tag = "logs")]
pub(crate) async fn delete_logs<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Json(request): Json<LogDeletionRequest>,
) -> Result<(StatusCode, Json<LogDeletionItem>), ApiError> {
    let context = command_context(&headers)?;
    let site = identifier("site", request.site, SiteId::new)?;
    let since = parse_time("since", request.since)?;
    let deletion = port(&state)?.delete(context, site, since).await?;
    Ok((StatusCode::ACCEPTED, Json(deletion.into())))
}

/// Deletions asked for, pending or applied, newest first.
#[utoipa::path(get, path = "/api/v1/logs/deletions", params(QueryHeaders),
    responses((status = 200, body = LogDeletionList)), tag = "logs")]
pub(crate) async fn list_log_deletions<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Json<LogDeletionList>, ApiError> {
    let scope = request_scope(&headers)?;
    let deletions = port(&state)?.deletions(scope).await?;
    Ok(Json(LogDeletionList {
        deletions: deletions.into_iter().map(Into::into).collect(),
    }))
}
