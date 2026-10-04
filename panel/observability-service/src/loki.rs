//! A client of Loki's HTTP API: range queries and delete requests.

use panel_errors::{PanelError, Result};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, time::Duration};

const TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug)]
pub struct Loki {
    client: reqwest::Client,
    base: String,
}

/// A record and the fields the store keeps with it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    /// Nanoseconds since the Unix epoch.
    pub time: i128,
    pub line: String,
    pub fields: BTreeMap<String, String>,
}

/// A delete request the store holds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Deletion {
    pub query: String,
    /// Seconds since the Unix epoch.
    pub start: i64,
    pub end: i64,
    pub created: i64,
    pub applied: bool,
}

#[derive(Deserialize)]
struct QueryResponse {
    data: QueryData,
}

#[derive(Deserialize)]
struct QueryData {
    #[serde(default)]
    result: Vec<Stream>,
}

#[derive(Deserialize)]
struct Stream {
    #[serde(default)]
    values: Vec<Vec<Value>>,
}

#[derive(Deserialize)]
struct DeleteRequest {
    query: String,
    start_time: Value,
    end_time: Value,
    #[serde(default)]
    created_at: Value,
    status: String,
}

fn unavailable(error: reqwest::Error) -> PanelError {
    PanelError::unavailable(format!("the log store did not answer: {error}"))
}

async fn refused(response: reqwest::Response) -> PanelError {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    PanelError::internal(format!(
        "the log store refused the request ({status}): {}",
        body.trim()
    ))
}

/// Seconds from a number or a numeric string, in seconds or milliseconds.
fn seconds(value: &Value) -> i64 {
    let number = match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.parse().ok(),
        _ => None,
    }
    .unwrap_or_default();
    let seconds = if number > 1e11 {
        number / 1000.0
    } else {
        number
    };
    seconds as i64
}

fn entry(value: Vec<Value>) -> Option<Entry> {
    let mut parts = value.into_iter();
    let time = parts.next()?.as_str()?.parse().ok()?;
    let line = parts.next()?.as_str()?.to_owned();
    let fields = parts
        .next()
        .and_then(|meta| meta.get("structuredMetadata").cloned())
        .and_then(|meta| serde_json::from_value::<BTreeMap<String, String>>(meta).ok())
        .unwrap_or_default();
    Some(Entry { time, line, fields })
}

impl Loki {
    pub fn new(url: &str) -> Result<Self> {
        let base = url.trim_end_matches('/').to_owned();
        if !(base.starts_with("http://") || base.starts_with("https://")) {
            return Err(PanelError::invalid_argument(format!(
                "{url:?} is not an http or https URL"
            )));
        }
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .build()
            .map_err(|error| {
                PanelError::internal(format!("cannot build an HTTP client: {error}"))
            })?;
        Ok(Self { client, base })
    }

    /// At most `limit` records of `query` in `[start, end)`, oldest first
    /// when `forward`, newest first otherwise.
    pub async fn query_range(
        &self,
        query: &str,
        start: i128,
        end: i128,
        limit: u32,
        forward: bool,
    ) -> Result<Vec<Entry>> {
        let response = self
            .client
            .get(format!("{}/loki/api/v1/query_range", self.base))
            .header("X-Loki-Response-Encoding-Flags", "categorize-labels")
            .query(&[
                ("query", query),
                ("start", &start.to_string()),
                ("end", &end.to_string()),
                ("limit", &limit.to_string()),
                ("direction", if forward { "forward" } else { "backward" }),
            ])
            .send()
            .await
            .map_err(unavailable)?;
        if !response.status().is_success() {
            return Err(refused(response).await);
        }
        let body: QueryResponse = response.json().await.map_err(|error| {
            PanelError::internal(format!("the log store answered unreadably: {error}"))
        })?;
        let mut entries: Vec<Entry> = body
            .data
            .result
            .into_iter()
            .flat_map(|stream| stream.values.into_iter().filter_map(entry))
            .collect();
        if forward {
            entries.sort_by_key(|entry| entry.time);
        } else {
            entries.sort_by_key(|entry| std::cmp::Reverse(entry.time));
        }
        entries.truncate(limit as usize);
        Ok(entries)
    }

    /// Asks the store to delete the records of `query` in `[start, end]`,
    /// in seconds.
    pub async fn delete(&self, query: &str, start: i64, end: i64) -> Result<()> {
        let response = self
            .client
            .post(format!("{}/loki/api/v1/delete", self.base))
            .query(&[
                ("query", query),
                ("start", &start.to_string()),
                ("end", &end.to_string()),
            ])
            .send()
            .await
            .map_err(unavailable)?;
        if !response.status().is_success() {
            return Err(refused(response).await);
        }
        Ok(())
    }

    /// The delete requests the store holds.
    pub async fn deletions(&self) -> Result<Vec<Deletion>> {
        let response = self
            .client
            .get(format!("{}/loki/api/v1/delete", self.base))
            .send()
            .await
            .map_err(unavailable)?;
        if !response.status().is_success() {
            return Err(refused(response).await);
        }
        let requests: Vec<DeleteRequest> = response.json().await.map_err(|error| {
            PanelError::internal(format!("the log store answered unreadably: {error}"))
        })?;
        Ok(requests
            .into_iter()
            .map(|request| Deletion {
                start: seconds(&request.start_time),
                end: seconds(&request.end_time),
                created: seconds(&request.created_at),
                applied: request.status == "processed",
                query: request.query,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn entries_carry_their_structured_metadata() {
        let parsed = entry(
            json!(["1791099815611523000", "line", {"structuredMetadata": {"pingora_panel_site_id": "shop"}}])
                .as_array()
                .unwrap()
                .clone(),
        )
        .unwrap();
        assert_eq!(parsed.time, 1_791_099_815_611_523_000);
        assert_eq!(parsed.fields["pingora_panel_site_id"], "shop");
        let bare = entry(json!(["1", "line"]).as_array().unwrap().clone()).unwrap();
        assert!(bare.fields.is_empty());
        assert!(entry(json!(["x", "line"]).as_array().unwrap().clone()).is_none());
    }

    #[test]
    fn times_are_read_in_seconds_or_milliseconds() {
        assert_eq!(seconds(&json!(1791096762)), 1_791_096_762);
        assert_eq!(seconds(&json!("1791096762.5")), 1_791_096_762);
        assert_eq!(seconds(&json!(1791096762123u64)), 1_791_096_762);
        assert_eq!(seconds(&Value::Null), 0);
    }

    #[test]
    fn only_http_urls_are_accepted() {
        assert!(Loki::new("http://127.0.0.1:3100/").is_ok());
        assert!(Loki::new("file:///etc/passwd").is_err());
    }
}
