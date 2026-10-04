//! `LogsPort` over `observability-service` (ADR 0026).

use crate::{time, ObservabilityClient};
use async_trait::async_trait;
use panel_application::{
    CommandContext, LogBatch, LogDeletion, LogDeletionState, LogFilter, LogKind, LogPage,
    LogRecord, LogSearch, LogTail, LogsPort, RequestScope, SiteId,
};
use panel_contracts::observability::v1::{self as wire, logs_client::LogsClient};
use panel_errors::Result;
use panel_service::command_context;
use panel_service::{propagate_trace, request_context, response_error, status_error};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio_stream::StreamExt;

fn timestamp(value: SystemTime) -> prost_types::Timestamp {
    value.into()
}

fn filter(value: &LogFilter) -> wire::LogFilter {
    let text = |value: &Option<String>| value.clone().unwrap_or_default();
    wire::LogFilter {
        kind: match value.kind {
            None => wire::LogKind::Unspecified,
            Some(LogKind::Error) => wire::LogKind::Error,
            Some(_) => wire::LogKind::Access,
        }
        .into(),
        site: value
            .site
            .as_ref()
            .map(|site| site.as_str().to_owned())
            .unwrap_or_default(),
        route: value
            .route
            .as_ref()
            .map(|route| route.as_str().to_owned())
            .unwrap_or_default(),
        status: text(&value.status),
        client: text(&value.client),
        path_prefix: text(&value.path_prefix),
        request_id: text(&value.request_id),
        text: text(&value.text),
    }
}

fn some(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

fn record(value: wire::LogRecord) -> Option<LogRecord> {
    Some(LogRecord {
        time: time(value.time)?,
        kind: if value.kind == i32::from(wire::LogKind::Error) {
            LogKind::Error
        } else {
            LogKind::Access
        },
        line: value.line,
        site: some(value.site),
        route: some(value.route),
        status: u16::try_from(value.status)
            .ok()
            .filter(|status| *status > 0),
        method: some(value.method),
        path: some(value.path),
        client: some(value.client),
        request_id: some(value.request_id),
        fields: value.fields.into_iter().collect(),
    })
}

fn deletion(value: wire::LogDeletion) -> Option<LogDeletion> {
    Some(LogDeletion {
        site: some(value.site),
        since: time(value.since).unwrap_or(UNIX_EPOCH),
        until: time(value.until)?,
        requested_at: time(value.requested_at).unwrap_or(UNIX_EPOCH),
        state: if value.state == i32::from(wire::LogDeletionState::Applied) {
            LogDeletionState::Applied
        } else {
            LogDeletionState::Pending
        },
    })
}

#[async_trait]
impl LogsPort for ObservabilityClient {
    async fn search(&self, scope: RequestScope, search: LogSearch) -> Result<LogPage> {
        let message = wire::LogsSearchRequest {
            context: Some(request_context(&scope)),
            filter: Some(filter(&search.filter)),
            since: search.since.map(timestamp),
            until: search.until.map(timestamp),
            limit: search.limit.unwrap_or_default(),
        };
        let response = LogsClient::new(self.channel.clone())
            .search(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(LogPage {
            records: response.records.into_iter().filter_map(record).collect(),
            next_until: time(response.next_until),
        })
    }

    async fn tail(
        &self,
        scope: RequestScope,
        filter_: LogFilter,
        after: Option<SystemTime>,
    ) -> Result<LogTail> {
        let message = wire::LogsTailRequest {
            context: Some(request_context(&scope)),
            filter: Some(filter(&filter_)),
            after: after.map(timestamp),
        };
        // A tail lasts as long as its reader, so it has no deadline.
        let mut request = tonic::Request::new(message);
        propagate_trace(request.metadata_mut(), scope.trace_context());
        let stream = LogsClient::new(self.channel.clone())
            .tail(request)
            .await
            .map_err(status_error)?
            .into_inner();
        Ok(Box::pin(stream.map(|message| {
            let message = message.map_err(status_error)?;
            response_error(message.error)?;
            Ok(LogBatch {
                records: message.records.into_iter().filter_map(record).collect(),
                cursor: time(message.cursor).unwrap_or(UNIX_EPOCH),
            })
        })))
    }

    async fn delete(
        &self,
        context: CommandContext,
        site: Option<SiteId>,
        since: Option<SystemTime>,
    ) -> Result<LogDeletion> {
        let scope = context.scope();
        let message = wire::LogsDeleteRequest {
            context: Some(command_context(&context)),
            site: site
                .map(|site| site.as_str().to_owned())
                .unwrap_or_default(),
            since: since.map(timestamp),
        };
        let response = LogsClient::new(self.channel.clone())
            .delete(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        response
            .deletion
            .and_then(deletion)
            .ok_or_else(|| panel_errors::PanelError::internal("the deletion was not described"))
    }

    async fn deletions(&self, scope: RequestScope) -> Result<Vec<LogDeletion>> {
        let message = wire::LogsListDeletionsRequest {
            context: Some(request_context(&scope)),
        };
        let response = LogsClient::new(self.channel.clone())
            .list_deletions(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(response
            .deletions
            .into_iter()
            .filter_map(deletion)
            .collect())
    }
}
