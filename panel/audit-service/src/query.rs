//! `pingora.panel.audit.v1.AuditQuery` over the audit store.

use crate::store::{Filter, PgAuditStore, Record};
use chrono::{DateTime, Utc};
use panel_contracts::audit::v1::{self as wire, audit_query_server::AuditQuery};
use tonic::{Request, Response, Status};

const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 500;

pub struct AuditQueryService {
    store: PgAuditStore,
}

impl AuditQueryService {
    pub fn new(store: PgAuditStore) -> Self {
        Self { store }
    }
}

fn timestamp(time: DateTime<Utc>) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: time.timestamp(),
        nanos: i32::try_from(time.timestamp_subsec_nanos()).unwrap_or_default(),
    }
}

fn time(value: Option<prost_types::Timestamp>) -> Option<DateTime<Utc>> {
    value.and_then(|value| {
        DateTime::from_timestamp(
            value.seconds,
            u32::try_from(value.nanos).unwrap_or_default(),
        )
    })
}

fn to_wire(record: Record) -> wire::AuditRecord {
    let entry = record.entry;
    wire::AuditRecord {
        sequence: entry.sequence,
        event_id: entry.event_id,
        source: entry.source,
        event_type: entry.event_type,
        event_version: entry.event_version,
        subject: entry.subject,
        occurred_at: Some(timestamp(entry.occurred_at)),
        recorded_at: Some(timestamp(entry.recorded_at)),
        actor_type: entry.actor_type,
        actor_id: entry.actor_id,
        correlation_id: entry.correlation_id,
        causation_id: entry.causation_id,
        idempotency_key: entry.idempotency_key,
        traceparent: entry.traceparent,
        data: entry.data.into_bytes(),
        hash: record.hash,
        previous_hash: record.previous_hash,
    }
}

#[tonic::async_trait]
impl AuditQuery for AuditQueryService {
    async fn list(
        &self,
        request: Request<wire::ListRequest>,
    ) -> Result<Response<wire::ListResponse>, Status> {
        let request = request.into_inner();
        let limit = match request.limit {
            0 => DEFAULT_LIMIT,
            limit => limit.min(MAX_LIMIT),
        };
        let filter = Filter {
            before: request.before,
            limit,
            actor_id: request.actor_id,
            event_type: request.event_type,
            subject: request.subject,
            correlation_id: request.correlation_id,
            since: time(request.since),
            until: time(request.until),
        };
        Ok(Response::new(match self.store.list(&filter).await {
            Ok(records) => {
                let next_before = (records.len() == limit as usize)
                    .then(|| records.last().map(|record| record.entry.sequence))
                    .flatten();
                wire::ListResponse {
                    records: records.into_iter().map(to_wire).collect(),
                    next_before,
                    error: None,
                }
            }
            Err(error) => wire::ListResponse {
                error: Some(error.into()),
                ..wire::ListResponse::default()
            },
        }))
    }

    async fn get(
        &self,
        request: Request<wire::GetRequest>,
    ) -> Result<Response<wire::GetResponse>, Status> {
        Ok(Response::new(
            match self.store.get(request.into_inner().sequence).await {
                Ok(record) => wire::GetResponse {
                    record: Some(to_wire(record)),
                    error: None,
                },
                Err(error) => wire::GetResponse {
                    record: None,
                    error: Some(error.into()),
                },
            },
        ))
    }

    async fn verify(
        &self,
        request: Request<wire::VerifyRequest>,
    ) -> Result<Response<wire::VerifyResponse>, Status> {
        let request = request.into_inner();
        Ok(Response::new(
            match self.store.verify(request.from, request.to).await {
                Ok(verification) => wire::VerifyResponse {
                    intact: verification.first_mismatch.is_none(),
                    checked: verification.checked,
                    first_mismatch: verification.first_mismatch,
                    head_sequence: verification.head_sequence,
                    head_hash: verification.head_hash,
                    error: None,
                },
                Err(error) => wire::VerifyResponse {
                    error: Some(error.into()),
                    ..wire::VerifyResponse::default()
                },
            },
        ))
    }
}
