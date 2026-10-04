//! The gateway's access and error logs, as the API reads and deletes them
//! (ADR 0026).

use crate::{CommandContext, Operation, OperationLog, RequestScope};
use async_trait::async_trait;
use futures_core::Stream;
use panel_domain::{RouteId, SiteId};
use panel_errors::Result;
use std::{collections::BTreeMap, pin::Pin, sync::Arc, time::SystemTime};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum LogKind {
    Access,
    Error,
}

/// Which records to read; unset fields match every record. The source
/// checks the values.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LogFilter {
    /// Both kinds when unset.
    pub kind: Option<LogKind>,
    pub site: Option<SiteId>,
    pub route: Option<RouteId>,
    /// A status code such as `502`, or a class such as `5xx`.
    pub status: Option<String>,
    /// A client address, or a CIDR block of them.
    pub client: Option<String>,
    /// Requests whose path starts with this.
    pub path_prefix: Option<String>,
    pub request_id: Option<String>,
    /// Text the record contains, ignoring case.
    pub text: Option<String>,
}

/// One page of a search.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LogSearch {
    pub filter: LogFilter,
    /// An hour before `until` when unset.
    pub since: Option<SystemTime>,
    /// Now when unset; the previous page's `next_until` reads the next page.
    pub until: Option<SystemTime>,
    /// The source's default when unset; it bounds the value.
    pub limit: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogRecord {
    pub time: SystemTime,
    pub kind: LogKind,
    /// The line as the gateway wrote it.
    pub line: String,
    pub site: Option<String>,
    pub route: Option<String>,
    pub status: Option<u16>,
    pub method: Option<String>,
    pub path: Option<String>,
    pub client: Option<String>,
    pub request_id: Option<String>,
    /// Every field the source keeps, by its name there.
    pub fields: BTreeMap<String, String>,
}

/// Records newest first.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LogPage {
    pub records: Vec<LogRecord>,
    /// Where the next page ends; unset after the last page.
    pub next_until: Option<SystemTime>,
}

/// Records that arrived, oldest first, and where to resume after them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogBatch {
    pub records: Vec<LogRecord>,
    pub cursor: SystemTime,
}

/// Batches until the stream is dropped; an error ends it, and the last
/// cursor received resumes it.
pub type LogTail = Pin<Box<dyn Stream<Item = Result<LogBatch>> + Send>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum LogDeletionState {
    /// Asked for; the records remain until the source applies it.
    Pending,
    Applied,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LogDeletion {
    /// Every site when unset.
    pub site: Option<String>,
    pub since: SystemTime,
    pub until: SystemTime,
    pub requested_at: SystemTime,
    pub state: LogDeletionState,
}

/// Reads and deletes the gateway's logs.
#[async_trait]
pub trait LogsPort: Send + Sync {
    async fn search(&self, scope: RequestScope, search: LogSearch) -> Result<LogPage>;

    /// Records after `after`, or from now when unset.
    async fn tail(
        &self,
        scope: RequestScope,
        filter: LogFilter,
        after: Option<SystemTime>,
    ) -> Result<LogTail>;

    /// Deletes the records of `site`, or every site, from `since`, or from
    /// the first, until now.
    async fn delete(
        &self,
        context: CommandContext,
        site: Option<SiteId>,
        since: Option<SystemTime>,
    ) -> Result<LogDeletion>;

    /// Newest first.
    async fn deletions(&self, scope: RequestScope) -> Result<Vec<LogDeletion>>;
}

/// A logs port that records each deletion, refused or not.
pub struct RecordedLogs {
    inner: Arc<dyn LogsPort>,
    log: Arc<dyn OperationLog>,
}

impl RecordedLogs {
    pub fn new(inner: Arc<dyn LogsPort>, log: Arc<dyn OperationLog>) -> Self {
        Self { inner, log }
    }
}

#[async_trait]
impl LogsPort for RecordedLogs {
    async fn search(&self, scope: RequestScope, search: LogSearch) -> Result<LogPage> {
        self.inner.search(scope, search).await
    }

    async fn tail(
        &self,
        scope: RequestScope,
        filter: LogFilter,
        after: Option<SystemTime>,
    ) -> Result<LogTail> {
        self.inner.tail(scope, filter, after).await
    }

    async fn delete(
        &self,
        context: CommandContext,
        site: Option<SiteId>,
        since: Option<SystemTime>,
    ) -> Result<LogDeletion> {
        let result = self
            .inner
            .delete(context.clone(), site.clone(), since)
            .await;
        let operation = Operation::DeleteLogs {
            site: site.as_ref().map(SiteId::as_str),
            result: result.as_ref(),
        };
        self.log.record(&context, operation).await;
        result
    }

    async fn deletions(&self, scope: RequestScope) -> Result<Vec<LogDeletion>> {
        self.inner.deletions(scope).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IdempotencyKey, RequestDeadline, RequestId};
    use panel_errors::PanelError;
    use std::{
        sync::Mutex,
        time::{Duration, UNIX_EPOCH},
    };

    struct Source;

    #[async_trait]
    impl LogsPort for Source {
        async fn search(&self, _: RequestScope, _: LogSearch) -> Result<LogPage> {
            Ok(LogPage::default())
        }

        async fn tail(
            &self,
            _: RequestScope,
            _: LogFilter,
            _: Option<SystemTime>,
        ) -> Result<LogTail> {
            Err(PanelError::unavailable("no tail here"))
        }

        async fn delete(
            &self,
            _: CommandContext,
            site: Option<SiteId>,
            _: Option<SystemTime>,
        ) -> Result<LogDeletion> {
            match site {
                Some(site) if site.as_str() == "locked" => Err(PanelError::precondition_failed(
                    "the store refuses deletions",
                )),
                site => Ok(LogDeletion {
                    site: site.map(|site| site.as_str().to_owned()),
                    since: UNIX_EPOCH,
                    until: UNIX_EPOCH + Duration::from_secs(60),
                    requested_at: UNIX_EPOCH + Duration::from_secs(60),
                    state: LogDeletionState::Pending,
                }),
            }
        }

        async fn deletions(&self, _: RequestScope) -> Result<Vec<LogDeletion>> {
            Ok(Vec::new())
        }
    }

    /// A deletion as the test log keeps it: the site, and its state or the
    /// code it was refused with.
    type Recorded = (Option<String>, std::result::Result<String, String>);

    #[derive(Default)]
    struct Recorder(Mutex<Vec<Recorded>>);

    #[async_trait]
    impl OperationLog for Recorder {
        async fn record(&self, _: &CommandContext, operation: Operation<'_>) {
            if let Operation::DeleteLogs { site, result } = operation {
                self.0.lock().unwrap().push((
                    site.map(str::to_owned),
                    result
                        .map(|deletion| format!("{:?}", deletion.state))
                        .map_err(|error| error.code.as_str().to_owned()),
                ));
            }
        }
    }

    fn context() -> CommandContext {
        CommandContext::new(
            RequestId::new("request-1").unwrap(),
            RequestId::new("request-1").unwrap(),
            "ops",
            RequestDeadline::new("2099-01-01T00:00:00Z").unwrap(),
            IdempotencyKey::new("key-1").unwrap(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn deletions_are_recorded_whether_they_succeed_or_not() {
        let recorder = Arc::new(Recorder::default());
        let logs = RecordedLogs::new(Arc::new(Source), recorder.clone());
        logs.delete(context(), Some(SiteId::new("shop").unwrap()), None)
            .await
            .unwrap();
        logs.delete(context(), Some(SiteId::new("locked").unwrap()), None)
            .await
            .unwrap_err();
        logs.delete(context(), None, None).await.unwrap();
        logs.search(context().scope(), LogSearch::default())
            .await
            .unwrap();
        assert_eq!(
            *recorder.0.lock().unwrap(),
            [
                (Some("shop".to_owned()), Ok("Pending".to_owned())),
                (
                    Some("locked".to_owned()),
                    Err("PRECONDITION_FAILED".to_owned())
                ),
                (None, Ok("Pending".to_owned())),
            ]
        );
    }
}
