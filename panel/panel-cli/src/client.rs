//! The public API as `ppanel` uses it: request identity, preconditions and
//! RFC 9457 problem details mapped onto stable exit codes.

use chrono::{SecondsFormat, Utc};
use reqwest::{header, Method, StatusCode};
use serde_json::Value;
use std::{fmt, process::ExitCode, time::Duration};
use uuid::Uuid;

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
    Api { status: StatusCode, problem: Value },
    Transport(String),
}

impl CliError {
    pub fn exit(&self) -> ExitCode {
        let code = match self {
            Self::Usage(_) => 2,
            Self::Transport(_) => Exit::Unavailable as u8,
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
            Self::Usage(message) => write!(formatter, "{message}"),
            Self::Transport(message) => write!(formatter, "cannot reach the API: {message}"),
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
                Ok(())
            }
        }
    }
}

pub type Result<T> = std::result::Result<T, CliError>;

pub struct Reply {
    pub body: Value,
    pub etag: Option<String>,
}

pub struct Api {
    http: reqwest::Client,
    base: String,
    actor: String,
    timeout: Duration,
    idempotency_key: Option<String>,
}

impl Api {
    pub fn new(
        base: &str,
        actor: String,
        timeout: Duration,
        idempotency_key: Option<String>,
    ) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(concat!("ppanel/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|error| CliError::Transport(error.to_string()))?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            actor,
            timeout,
            idempotency_key,
        })
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

    /// A change, with the identity, deadline and idempotency key the API requires.
    pub async fn change(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        if_match: Option<&str>,
    ) -> Result<Reply> {
        let deadline = Utc::now()
            + chrono::Duration::from_std(self.timeout).unwrap_or(chrono::Duration::seconds(30));
        let key = self
            .idempotency_key
            .clone()
            .unwrap_or_else(|| Uuid::now_v7().to_string());
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base))
            .header("x-request-id", Uuid::now_v7().to_string())
            .header("x-actor", &self.actor)
            .header(
                "x-deadline",
                deadline.to_rfc3339_opts(SecondsFormat::Secs, true),
            )
            .header("idempotency-key", key);
        if let Some(body) = body {
            request = request.json(body);
        }
        if let Some(etag) = if_match {
            request = request.header(header::IF_MATCH, etag);
        }
        self.execute(request).await
    }

    async fn execute(&self, request: reqwest::RequestBuilder) -> Result<Reply> {
        let response = request
            .send()
            .await
            .map_err(|error| CliError::Transport(error.to_string()))?;
        let status = response.status();
        let etag = response
            .headers()
            .get(header::ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let bytes = response
            .bytes()
            .await
            .map_err(|error| CliError::Transport(error.to_string()))?;
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        if status.is_success() {
            Ok(Reply { body, etag })
        } else {
            Err(CliError::Api {
                status,
                problem: body,
            })
        }
    }
}
