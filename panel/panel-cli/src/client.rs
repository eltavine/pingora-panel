//! The public API as `ppanel` uses it: request identity, preconditions and
//! RFC 9457 problem details mapped onto stable exit codes.

use chrono::{SecondsFormat, Utc};
use reqwest::{header, Method, StatusCode};
use serde_json::Value;
use std::{fmt, process::ExitCode, time::Duration};
use tokio_tungstenite::{
    tungstenite::{
        handshake::{client::generate_key, derive_accept_key},
        protocol::Role,
    },
    WebSocketStream,
};
use uuid::Uuid;

/// The furthest deadline a change names; a longer timeout still waits for the
/// answer.
const LONGEST_DEADLINE: Duration = Duration::from_secs(24 * 60 * 60);

/// Process exit codes; scripts may rely on them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Exit {
    Failure = 1,
    NotFound = 3,
    /// The target changed or a precondition failed; read it again and retry.
    Conflict = 4,
    /// The request was refused as invalid.
    Rejected = 5,
    /// The service is unavailable or overloaded; retry later.
    Unavailable = 6,
    Denied = 7,
}

#[derive(Debug)]
pub(crate) enum CliError {
    Usage(String),
    Api {
        status: StatusCode,
        problem: Value,
    },
    Transport(String),
    /// A check that ran and found a problem.
    Failed(String),
    /// A stream the API ended with an error code, such as
    /// `RESOURCE_EXHAUSTED`.
    Ended {
        code: String,
        message: String,
    },
}

impl CliError {
    /// A transport failure with its causes, such as a certificate the
    /// system does not trust.
    pub fn transport(error: impl std::error::Error) -> Self {
        let mut message = error.to_string();
        let mut cause = error.source();
        while let Some(source) = cause {
            message.push_str(": ");
            message.push_str(&source.to_string());
            cause = source.source();
        }
        Self::Transport(message)
    }

    pub fn exit(&self) -> ExitCode {
        let code = match self {
            Self::Usage(_) => 2,
            Self::Failed(_) => Exit::Failure as u8,
            Self::Transport(_) => Exit::Unavailable as u8,
            Self::Ended { code, .. } => {
                (match code.as_str() {
                    "NOT_FOUND" => Exit::NotFound,
                    "CONFLICT" | "PRECONDITION_FAILED" => Exit::Conflict,
                    "INVALID_ARGUMENT" | "VALIDATION_FAILED" => Exit::Rejected,
                    "UNAUTHENTICATED" | "PERMISSION_DENIED" => Exit::Denied,
                    "UNAVAILABLE"
                    | "STORAGE_UNAVAILABLE"
                    | "RESOURCE_EXHAUSTED"
                    | "DEADLINE_EXCEEDED" => Exit::Unavailable,
                    _ => Exit::Failure,
                }) as u8
            }
            Self::Api { status, .. } => {
                (match status.as_u16() {
                    404 => Exit::NotFound,
                    409 | 412 | 428 => Exit::Conflict,
                    400 | 413 | 415 | 422 => Exit::Rejected,
                    401 | 403 => Exit::Denied,
                    408 | 429 | 502..=504 => Exit::Unavailable,
                    _ => Exit::Failure,
                }) as u8
            }
        };
        ExitCode::from(code)
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(message) | Self::Failed(message) => write!(formatter, "{message}"),
            Self::Transport(message) => write!(formatter, "cannot reach the API: {message}"),
            Self::Ended { message, .. } => write!(formatter, "{message}"),
            Self::Api { status, problem } => {
                let detail = problem["detail"]
                    .as_str()
                    .or_else(|| problem["title"].as_str())
                    .unwrap_or("the request failed");
                write!(formatter, "{} {detail}", status.as_u16())?;
                if let Some(errors) = problem["field_errors"]
                    .as_array()
                    .or_else(|| problem["diagnostics"].as_array())
                {
                    for error in errors {
                        let resource = error["source_span"]
                            .as_str()
                            .or_else(|| error["resource_id"].as_str())
                            .or_else(|| error["field"].as_str())
                            .unwrap_or("");
                        let message = error["message"].as_str().unwrap_or("");
                        write!(formatter, "\n  {resource}: {message}")?;
                    }
                }
                if let Some(request) = problem["request_id"].as_str() {
                    write!(formatter, "\n  request {request}")?;
                }
                if *status == StatusCode::UNAUTHORIZED {
                    write!(
                        formatter,
                        "\n  log in with `ppanel login`, or pass an API token with --token"
                    )?;
                }
                Ok(())
            }
        }
    }
}

pub type Result<T> = std::result::Result<T, CliError>;

pub struct Reply {
    pub body: Value,
    pub etag: Option<String>,
    pub status: StatusCode,
}

pub struct Api {
    http: reqwest::Client,
    base: String,
    /// An API token or a session secret, sent as a bearer credential.
    credential: Option<String>,
    timeout: Duration,
    idempotency_key: Option<String>,
}

impl Api {
    pub fn new(
        base: &str,
        credential: Option<String>,
        timeout: Duration,
        idempotency_key: Option<String>,
    ) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(concat!("ppanel/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(CliError::transport)?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            credential,
            timeout,
            idempotency_key,
        })
    }

    /// The API's base URL, which keys stored sessions.
    pub fn base(&self) -> &str {
        &self.base
    }

    pub async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Reply> {
        let request = self.http.get(format!("{}{path}", self.base)).query(query);
        self.execute(request.header("x-request-id", Uuid::now_v7().to_string()))
            .await
    }

    pub async fn post_read(&self, path: &str, body: &Value) -> Result<Reply> {
        let request = self.http.post(format!("{}{path}", self.base)).json(body);
        self.execute(request.header("x-request-id", Uuid::now_v7().to_string()))
            .await
    }

    /// A command lasting at most `lasting`, with the identity, deadline and
    /// idempotency key the API requires.
    fn command(&self, method: Method, path: &str, lasting: Duration) -> reqwest::RequestBuilder {
        let deadline = Utc::now()
            + chrono::Duration::from_std(lasting.min(LONGEST_DEADLINE))
                .unwrap_or(chrono::Duration::seconds(30));
        let key = self
            .idempotency_key
            .clone()
            .unwrap_or_else(|| Uuid::now_v7().to_string());
        self.http
            .request(method, format!("{}{path}", self.base))
            .header("x-request-id", Uuid::now_v7().to_string())
            .header(
                "x-deadline",
                deadline.to_rfc3339_opts(SecondsFormat::Secs, true),
            )
            .header("idempotency-key", key)
    }

    /// A change.
    pub async fn change(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        if_match: Option<&str>,
    ) -> Result<Reply> {
        let mut request = self.command(method, path, self.timeout);
        if let Some(body) = body {
            request = request.json(body);
        }
        if let Some(etag) = if_match {
            request = request.header(header::IF_MATCH, etag);
        }
        self.execute(request).await
    }

    /// A read whose body is not JSON, such as a log file, as it arrives.
    pub async fn stream(
        &self,
        path: &str,
        query: &[(&str, String)],
        timeout: Duration,
    ) -> Result<reqwest::Response> {
        let request = self
            .http
            .get(format!("{}{path}", self.base))
            .query(query)
            .timeout(timeout)
            .header("x-request-id", Uuid::now_v7().to_string());
        let response = self
            .authorized(request)
            .send()
            .await
            .map_err(CliError::transport)?;
        if response.status().is_success() {
            Ok(response)
        } else {
            Err(refused(response).await)
        }
    }

    /// A change the API answers as server-sent events, read as they arrive
    /// for at most `lasting`.
    pub async fn change_events(
        &self,
        path: &str,
        body: &Value,
        lasting: Duration,
    ) -> Result<reqwest::Response> {
        let request = self
            .command(Method::POST, path, lasting)
            .timeout(lasting)
            .header(header::ACCEPT, "text/event-stream")
            .json(body);
        let response = self
            .authorized(request)
            .send()
            .await
            .map_err(CliError::transport)?;
        if response.status().is_success() {
            Ok(response)
        } else {
            Err(refused(response).await)
        }
    }

    /// A WebSocket (RFC 6455) opened over the API's own HTTP client, so it
    /// is trusted and authenticated as every other request is.
    pub async fn websocket(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<WebSocketStream<reqwest::Upgraded>> {
        let key = generate_key();
        let request = self
            .http
            .get(format!("{}{path}", self.base))
            .query(query)
            .header(header::CONNECTION, "upgrade")
            .header(header::UPGRADE, "websocket")
            .header(header::SEC_WEBSOCKET_VERSION, "13")
            .header(header::SEC_WEBSOCKET_KEY, &key)
            .header("x-request-id", Uuid::now_v7().to_string());
        let response = self
            .authorized(request)
            .send()
            .await
            .map_err(CliError::transport)?;
        if response.status() != StatusCode::SWITCHING_PROTOCOLS {
            return Err(refused(response).await);
        }
        let accept = derive_accept_key(key.as_bytes());
        let accepted = response
            .headers()
            .get(header::SEC_WEBSOCKET_ACCEPT)
            .and_then(|value| value.to_str().ok());
        if accepted != Some(accept.as_str()) {
            return Err(CliError::Transport(
                "the API did not accept the WebSocket handshake".into(),
            ));
        }
        let upgraded = response.upgrade().await.map_err(CliError::transport)?;
        Ok(WebSocketStream::from_raw_socket(upgraded, Role::Client, None).await)
    }

    fn authorized(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.credential {
            Some(credential) => request.bearer_auth(credential),
            None => request,
        }
    }

    async fn execute(&self, request: reqwest::RequestBuilder) -> Result<Reply> {
        let response = self
            .authorized(request)
            .send()
            .await
            .map_err(CliError::transport)?;
        let status = response.status();
        let etag = response
            .headers()
            .get(header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let bytes = response.bytes().await.map_err(CliError::transport)?;
        let body = body(&bytes);
        if status.is_success() {
            Ok(Reply { body, etag, status })
        } else {
            Err(CliError::Api {
                status,
                problem: body,
            })
        }
    }
}

fn body(bytes: &[u8]) -> Value {
    if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(bytes).into_owned()))
    }
}

async fn refused(response: reqwest::Response) -> CliError {
    let status = response.status();
    match response.bytes().await {
        Ok(bytes) => CliError::Api {
            status,
            problem: body(&bytes),
        },
        Err(error) => CliError::transport(error),
    }
}
