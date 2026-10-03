//! The audit trail: every change and every refused or failed attempt,
//! newest first, and verification of its hash chain.

use crate::{
    error::ApiError,
    request_context::{request_scope, QueryHeaders},
    ApiState,
};
use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    Json,
};
use chrono::{DateTime, SecondsFormat, Utc};
use panel_application::{AuditFilter, AuditPort, AuditRecord, AuditVerification};
use panel_errors::PanelError;
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::SystemTime};
use utoipa::{IntoParams, ToSchema};

fn port<U>(state: &ApiState<U>) -> Result<Arc<dyn AuditPort>, ApiError> {
    state.audit.clone().ok_or_else(|| {
        ApiError::new(PanelError::unavailable(
            "the audit trail is not available here",
        ))
    })
}

fn rfc3339(time: Option<SystemTime>) -> Option<String> {
    time.map(|time| DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Micros, true))
}

fn parse_time(name: &str, value: Option<&String>) -> Result<Option<SystemTime>, ApiError> {
    value
        .map(|value| {
            DateTime::parse_from_rfc3339(value)
                .map(|time| SystemTime::from(time.with_timezone(&Utc)))
                .map_err(|_| {
                    ApiError::new(PanelError::invalid_argument(format!(
                        "{name} must be an RFC 3339 time"
                    )))
                })
        })
        .transpose()
}

/// One audited event.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AuditEvent {
    /// Position in the hash chain, from 1.
    pub sequence: u64,
    pub event_id: String,
    /// The producing service, as a CloudEvents source.
    pub source: String,
    /// For example `config.draft.changed` or `gateway.reloaded`.
    pub event_type: String,
    pub event_version: u32,
    /// `<aggregate type>/<aggregate id>`.
    pub subject: String,
    pub occurred_at: Option<String>,
    pub recorded_at: Option<String>,
    /// How the actor was identified, such as `user`.
    pub actor_type: String,
    pub actor_id: String,
    pub correlation_id: String,
    pub causation_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub idempotency_key: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub traceparent: String,
    #[schema(value_type = Object)]
    pub data: serde_json::Value,
    /// Hex SHA-256 of the previous hash and this record.
    pub hash: String,
    /// Empty for the first record.
    pub previous_hash: String,
}

impl From<AuditRecord> for AuditEvent {
    fn from(value: AuditRecord) -> Self {
        Self {
            sequence: value.sequence,
            event_id: value.event_id,
            source: value.source,
            event_type: value.event_type,
            event_version: value.event_version,
            subject: value.subject,
            occurred_at: rfc3339(value.occurred_at),
            recorded_at: rfc3339(value.recorded_at),
            actor_type: value.actor_type,
            actor_id: value.actor_id,
            correlation_id: value.correlation_id,
            causation_id: value.causation_id,
            idempotency_key: value.idempotency_key,
            traceparent: value.traceparent,
            data: value.data,
            hash: value.hash,
            previous_hash: value.previous_hash,
        }
    }
}

/// Events newest first.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AuditEventPage {
    pub items: Vec<AuditEvent>,
    /// Pass as `before` for the next page; absent on the last.
    pub next_before: Option<u64>,
}

/// Whether the hash chain is intact.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AuditVerificationResponse {
    pub intact: bool,
    pub checked: u64,
    /// The first record that does not match, when one does not.
    pub first_mismatch: Option<u64>,
    pub head_sequence: u64,
    pub head_hash: String,
}

impl From<AuditVerification> for AuditVerificationResponse {
    fn from(value: AuditVerification) -> Self {
        Self {
            intact: value.intact,
            checked: value.checked,
            first_mismatch: value.first_mismatch,
            head_sequence: value.head_sequence,
            head_hash: value.head_hash,
        }
    }
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct AuditQuery {
    /// Only events older than this sequence.
    before: Option<u64>,
    /// At most this many, 50 by default and 500 at most.
    limit: Option<u32>,
    /// Who acted.
    actor: Option<String>,
    /// An event type, or a prefix ending in `.` such as `config.`.
    #[serde(rename = "type")]
    #[param(rename = "type")]
    event_type: Option<String>,
    /// What was acted on, such as `configuration/draft`.
    subject: Option<String>,
    /// Every event of one request or the requests it caused.
    correlation_id: Option<String>,
    /// Events at or after this RFC 3339 time.
    since: Option<String>,
    /// Events before this RFC 3339 time.
    until: Option<String>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct VerifyQuery {
    /// The first event to check; the whole chain by default.
    from: Option<u64>,
    /// The last event to check; the newest by default.
    to: Option<u64>,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct SequencePath {
    /// Position in the hash chain.
    sequence: u64,
}

/// Audit events, newest first, filtered by actor, type, subject,
/// correlation and time.
#[utoipa::path(get, path = "/api/v1/audit-events", params(QueryHeaders, AuditQuery),
    responses((status = 200, body = AuditEventPage)), tag = "audit")]
pub(crate) async fn list_audit_events<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Query(query): Query<AuditQuery>,
) -> Result<Json<AuditEventPage>, ApiError> {
    let scope = request_scope(&headers)?;
    let filter = AuditFilter {
        before: query.before,
        limit: query.limit.unwrap_or_default(),
        actor_id: query.actor.unwrap_or_default(),
        event_type: query.event_type.unwrap_or_default(),
        subject: query.subject.unwrap_or_default(),
        correlation_id: query.correlation_id.unwrap_or_default(),
        since: parse_time("since", query.since.as_ref())?,
        until: parse_time("until", query.until.as_ref())?,
    };
    let page = port(&state)?.list(scope, filter).await?;
    Ok(Json(AuditEventPage {
        items: page.records.into_iter().map(AuditEvent::from).collect(),
        next_before: page.next_before,
    }))
}

/// One audit event.
#[utoipa::path(get, path = "/api/v1/audit-events/{sequence}", params(QueryHeaders, SequencePath),
    responses((status = 200, body = AuditEvent)), tag = "audit")]
pub(crate) async fn get_audit_event<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<SequencePath>,
) -> Result<Json<AuditEvent>, ApiError> {
    let scope = request_scope(&headers)?;
    Ok(Json(port(&state)?.get(scope, path.sequence).await?.into()))
}

/// Recomputes the hash chain and checks it against its links, checkpoints
/// and head.
#[utoipa::path(get, path = "/api/v1/audit-events/verify", params(QueryHeaders, VerifyQuery),
    responses((status = 200, body = AuditVerificationResponse)), tag = "audit")]
pub(crate) async fn verify_audit_events<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Query(query): Query<VerifyQuery>,
) -> Result<Json<AuditVerificationResponse>, ApiError> {
    let scope = request_scope(&headers)?;
    Ok(Json(
        port(&state)?
            .verify(scope, query.from, query.to)
            .await?
            .into(),
    ))
}
