//! `pingora.panel.observability.v1.Logs` over Loki (ADR 0026).

use crate::{
    logql::{self, Filter, ERROR_EVENT},
    loki::{Entry, Loki},
};
use panel_contracts::observability::v1::{self as wire, logs_server};
use panel_domain::SiteId;
use panel_errors::{PanelError, Result};
use std::{
    collections::HashMap,
    pin::Pin,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::mpsc;
use tokio_stream::{wrappers::ReceiverStream, Stream};
use tonic::{Request, Response, Status};

const HOUR: i128 = 3_600 * 1_000_000_000;
const DEFAULT_PAGE: u32 = 100;
const MAX_PAGE: u32 = 500;
/// How often a tail asks for new records.
const POLL: Duration = Duration::from_secs(1);
/// How long a record may take to reach the store; a tail reads only older
/// ones, so late records are not skipped.
const SETTLE: i128 = 2 * 1_000_000_000;
/// Records a tail reads at a time.
const BATCH: u32 = 500;
/// Messages a tail buffers: two batches, and room for the message that
/// ends it.
const BUFFER: usize = 3;
/// Fields of the store that describe the collector, not the request.
const HIDDEN: &str = "log_file_";

type TailStream =
    Pin<Box<dyn Stream<Item = std::result::Result<wire::LogsTailResponse, Status>> + Send>>;

pub struct LogsService {
    loki: Loki,
}

fn now() -> i128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos() as i128)
}

fn timestamp(nanos: i128) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: nanos.div_euclid(1_000_000_000) as i64,
        nanos: nanos.rem_euclid(1_000_000_000) as i32,
    }
}

fn nanos(timestamp: prost_types::Timestamp) -> Result<i128> {
    if !(0..1_000_000_000).contains(&timestamp.nanos) {
        return Err(PanelError::invalid_argument(
            "a time has invalid nanoseconds",
        ));
    }
    Ok(i128::from(timestamp.seconds) * 1_000_000_000 + i128::from(timestamp.nanos))
}

fn record(entry: Entry) -> wire::LogRecord {
    let field = |name: &str| entry.fields.get(name).cloned().unwrap_or_default();
    let kind = if entry.fields.get("event_name").map(String::as_str) == Some(ERROR_EVENT) {
        wire::LogKind::Error
    } else {
        wire::LogKind::Access
    };
    wire::LogRecord {
        time: Some(timestamp(entry.time)),
        kind: kind.into(),
        site: field("pingora_panel_site_id"),
        route: field("pingora_panel_route_id"),
        status: field("http_response_status_code")
            .parse()
            .unwrap_or_default(),
        method: field("http_request_method"),
        path: field("url_path"),
        client: field("client_address"),
        request_id: field("pingora_panel_request_id"),
        fields: entry
            .fields
            .iter()
            .filter(|(name, _)| !name.starts_with(HIDDEN))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect::<HashMap<_, _>>(),
        line: entry.line,
    }
}

fn site(value: String) -> Result<Option<SiteId>> {
    (!value.is_empty())
        .then(|| {
            SiteId::new(value).map_err(|error| PanelError::invalid_argument(error.to_string()))
        })
        .transpose()
}

impl LogsService {
    pub fn new(loki: Loki) -> Self {
        Self { loki }
    }

    async fn search(&self, request: wire::LogsSearchRequest) -> Result<wire::LogsSearchResponse> {
        let filter = Filter::from_wire(request.filter)?;
        let now = now();
        let until = request
            .until
            .map(nanos)
            .transpose()?
            .unwrap_or(now)
            .min(now);
        let since = request
            .since
            .map(nanos)
            .transpose()?
            .unwrap_or(until - HOUR);
        if since >= until {
            return Err(PanelError::invalid_argument("since must come before until"));
        }
        let limit = match request.limit {
            0 => DEFAULT_PAGE,
            limit => limit.min(MAX_PAGE),
        };
        let entries = self
            .loki
            .query_range(&filter.query(), since, until, limit, false)
            .await?;
        let next_until = (entries.len() == limit as usize)
            .then(|| entries.last().map(|entry| timestamp(entry.time)))
            .flatten();
        Ok(wire::LogsSearchResponse {
            records: entries.into_iter().map(record).collect(),
            next_until,
            error: None,
        })
    }

    async fn delete(&self, request: wire::LogsDeleteRequest) -> Result<wire::LogDeletion> {
        let site = site(request.site)?;
        let since = request.since.map(nanos).transpose()?.unwrap_or(0);
        // The store refuses deletions that end in the future.
        let until = now().div_euclid(1_000_000_000) * 1_000_000_000;
        if since >= until {
            return Err(PanelError::invalid_argument("since must be in the past"));
        }
        let query = Filter::of_site(site.clone()).query();
        self.loki
            .delete(
                &query,
                since.div_euclid(1_000_000_000) as i64,
                until.div_euclid(1_000_000_000) as i64,
            )
            .await?;
        Ok(wire::LogDeletion {
            site: site
                .map(|site| site.as_str().to_owned())
                .unwrap_or_default(),
            since: Some(timestamp(since)),
            until: Some(timestamp(until)),
            requested_at: Some(timestamp(now())),
            state: wire::LogDeletionState::Pending.into(),
        })
    }

    async fn deletions(&self) -> Result<Vec<wire::LogDeletion>> {
        let mut deletions: Vec<_> = self
            .loki
            .deletions()
            .await?
            .into_iter()
            .filter_map(|deletion| {
                let site = logql::deletion_site(&deletion.query)?;
                let second = |seconds: i64| Some(timestamp(i128::from(seconds) * 1_000_000_000));
                Some(wire::LogDeletion {
                    site: site.unwrap_or_default(),
                    since: second(deletion.start),
                    until: second(deletion.end),
                    requested_at: second(deletion.created),
                    state: if deletion.applied {
                        wire::LogDeletionState::Applied
                    } else {
                        wire::LogDeletionState::Pending
                    }
                    .into(),
                })
            })
            .collect();
        deletions.sort_by_key(|deletion| {
            std::cmp::Reverse(deletion.requested_at.map(|time| (time.seconds, time.nanos)))
        });
        Ok(deletions)
    }
}

fn failure(error: PanelError, cursor: i128) -> wire::LogsTailResponse {
    wire::LogsTailResponse {
        records: Vec::new(),
        cursor: Some(timestamp(cursor)),
        error: Some(error.into()),
    }
}

/// Sends what `query` finds after `after` until the client goes away, the
/// store fails or the client falls behind.
async fn follow(
    loki: Loki,
    query: String,
    mut after: i128,
    sender: mpsc::Sender<std::result::Result<wire::LogsTailResponse, Status>>,
) {
    let mut ticks = tokio::time::interval(POLL);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticks.tick().await;
        loop {
            if sender.is_closed() {
                return;
            }
            let until = now() - SETTLE;
            if until <= after + 1 {
                break;
            }
            let entries = match loki
                .query_range(&query, after + 1, until, BATCH, true)
                .await
            {
                Ok(entries) => entries,
                Err(error) => {
                    let _ = sender.send(Ok(failure(error, after))).await;
                    return;
                }
            };
            let Some(last) = entries.last().map(|entry| entry.time) else {
                break;
            };
            let full = entries.len() == BATCH as usize;
            if sender.capacity() <= 1 {
                let behind = PanelError::resource_exhausted(
                    "the client fell behind the logs; resume from the last cursor",
                );
                let _ = sender.send(Ok(failure(behind, after))).await;
                return;
            }
            after = last;
            let message = wire::LogsTailResponse {
                records: entries.into_iter().map(record).collect(),
                cursor: Some(timestamp(after)),
                error: None,
            };
            if sender.send(Ok(message)).await.is_err() {
                return;
            }
            if !full {
                break;
            }
        }
    }
}

#[tonic::async_trait]
impl logs_server::Logs for LogsService {
    async fn search(
        &self,
        request: Request<wire::LogsSearchRequest>,
    ) -> std::result::Result<Response<wire::LogsSearchResponse>, Status> {
        Ok(Response::new(
            LogsService::search(self, request.into_inner())
                .await
                .unwrap_or_else(|error| wire::LogsSearchResponse {
                    error: Some(error.into()),
                    ..Default::default()
                }),
        ))
    }

    type TailStream = TailStream;

    async fn tail(
        &self,
        request: Request<wire::LogsTailRequest>,
    ) -> std::result::Result<Response<Self::TailStream>, Status> {
        let request = request.into_inner();
        let (sender, receiver) = mpsc::channel(BUFFER);
        let start = now();
        let parsed = Filter::from_wire(request.filter).and_then(|filter| {
            let after = request.after.map(nanos).transpose()?.unwrap_or(start);
            Ok((filter, after))
        });
        match parsed {
            Ok((filter, after)) => {
                tokio::spawn(follow(self.loki.clone(), filter.query(), after, sender));
            }
            Err(error) => {
                let _ = sender.try_send(Ok(failure(error, start)));
            }
        }
        Ok(Response::new(Box::pin(ReceiverStream::new(receiver))))
    }

    async fn delete(
        &self,
        request: Request<wire::LogsDeleteRequest>,
    ) -> std::result::Result<Response<wire::LogsDeleteResponse>, Status> {
        Ok(Response::new(
            match LogsService::delete(self, request.into_inner()).await {
                Ok(deletion) => wire::LogsDeleteResponse {
                    deletion: Some(deletion),
                    error: None,
                },
                Err(error) => wire::LogsDeleteResponse {
                    deletion: None,
                    error: Some(error.into()),
                },
            },
        ))
    }

    async fn list_deletions(
        &self,
        _request: Request<wire::LogsListDeletionsRequest>,
    ) -> std::result::Result<Response<wire::LogsListDeletionsResponse>, Status> {
        Ok(Response::new(match self.deletions().await {
            Ok(deletions) => wire::LogsListDeletionsResponse {
                deletions,
                error: None,
            },
            Err(error) => wire::LogsListDeletionsResponse {
                deletions: Vec::new(),
                error: Some(error.into()),
            },
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_convert_both_ways() {
        let time = 1_791_099_815_611_523_000;
        assert_eq!(nanos(timestamp(time)).unwrap(), time);
        assert!(nanos(prost_types::Timestamp {
            seconds: 0,
            nanos: -1
        })
        .is_err());
    }

    #[test]
    fn records_name_their_kind_and_hide_collector_fields() {
        let entry = Entry {
            time: 1,
            line: "line".into(),
            fields: [
                ("event_name", ERROR_EVENT),
                ("http_response_status_code", "502"),
                ("pingora_panel_site_id", "shop"),
                ("log_file_path", "/var/log/pingora-panel/error.log"),
            ]
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value.to_owned()))
            .collect(),
        };
        let record = record(entry);
        assert_eq!(record.kind, i32::from(wire::LogKind::Error));
        assert_eq!(record.status, 502);
        assert_eq!(record.site, "shop");
        assert!(!record.fields.contains_key("log_file_path"));
    }
}
