//! Alert rules, the channels they notify and the notifications sent
//! (ADR 0027).

use crate::{
    error::ApiError,
    request_context::{command_context, request_scope, MutationHeaders, QueryHeaders},
    ApiState,
};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use chrono::{DateTime, SecondsFormat, Utc};
use panel_application::{
    AlertChannel, AlertChannelKind, AlertChannelSecret, AlertComparison, AlertMeasure,
    AlertNotification, AlertNotificationKind, AlertNotificationQuery, AlertNotificationState,
    AlertRule, AlertRuleSpec, AlertSeverity, AlertState, AlertTest, AlertsPort, NewAlertChannel,
};
use panel_domain::{RouteId, SiteId, UpstreamPoolId};
use panel_errors::PanelError;
use serde::{Deserialize, Serialize};
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};
use utoipa::{IntoParams, ToSchema};
use zeroize::Zeroizing;

fn port<U>(state: &ApiState<U>) -> Result<Arc<dyn AlertsPort>, ApiError> {
    state
        .alerts
        .clone()
        .ok_or_else(|| ApiError::new(PanelError::unavailable("alerts are not available here")))
}

fn rfc3339(time: SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn etag(version: u64) -> String {
    format!("\"{version}\"")
}

/// The version `If-Match` names, if it was sent.
fn if_match(headers: &HeaderMap) -> Result<Option<u64>, ApiError> {
    let Some(value) = headers.get(header::IF_MATCH) else {
        return Ok(None);
    };
    value
        .to_str()
        .ok()
        .and_then(|value| value.strip_prefix('"')?.strip_suffix('"')?.parse().ok())
        .map(Some)
        .ok_or_else(|| {
            ApiError::new(PanelError::invalid_argument(
                "If-Match carries the ETag the resource was read with",
            ))
        })
}

fn required_match(headers: &HeaderMap, what: &str) -> Result<u64, ApiError> {
    if_match(headers)?.ok_or_else(|| {
        ApiError::new(PanelError::precondition_required(format!(
            "send If-Match with the ETag of the {what} being changed"
        )))
    })
}

fn tagged<T: Serialize>(status: StatusCode, version: u64, body: T) -> Response {
    let mut response = (status, Json(body)).into_response();
    if let Ok(value) = HeaderValue::from_str(&etag(version)) {
        response.headers_mut().insert(header::ETAG, value);
    }
    response
}

/// What a rule reads, over the last five minutes.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AlertMeasureName {
    /// The share of requests answered with a 5xx status, from 0 to 1.
    ServerErrorRatio,
    /// The 95th percentile request latency, in seconds.
    LatencyP95,
    /// Requests per second; no requests read as zero.
    RequestRate,
    /// The share of failed upstream attempts, from 0 to 1.
    UpstreamErrorRatio,
    /// The gateway's open client connections.
    OpenConnections,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AlertComparisonName {
    Above,
    Below,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AlertSeverityName {
    Warning,
    Critical,
}

#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AlertStateName {
    Inactive,
    /// The condition holds, for less than the pending period so far.
    Pending,
    Firing,
}

fn enabled() -> bool {
    true
}

/// What a rule watches and whom it tells.
#[derive(Clone, Debug, Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AlertRuleSpecBody {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub measure: AlertMeasureName,
    pub comparison: AlertComparisonName,
    pub threshold: f64,
    /// Seconds the condition holds before the rule fires, at most a day;
    /// 0 fires at once.
    #[serde(default)]
    pub pending_seconds: u64,
    /// Request measures: one site, or every site when absent.
    #[serde(default)]
    pub site: Option<String>,
    /// Request measures: one route of `site`.
    #[serde(default)]
    pub route: Option<String>,
    /// The upstream measure: one upstream, or every upstream when absent.
    #[serde(default)]
    pub upstream: Option<String>,
    pub severity: AlertSeverityName,
    #[serde(default = "enabled")]
    pub enabled: bool,
    /// The channels notified when the rule fires and resolves.
    #[serde(default)]
    pub channels: Vec<String>,
}

fn scope_id<T, E>(
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

impl AlertRuleSpecBody {
    fn spec(self) -> Result<AlertRuleSpec, ApiError> {
        Ok(AlertRuleSpec {
            name: self.name,
            description: self.description,
            measure: match self.measure {
                AlertMeasureName::ServerErrorRatio => AlertMeasure::ServerErrorRatio,
                AlertMeasureName::LatencyP95 => AlertMeasure::LatencyP95,
                AlertMeasureName::RequestRate => AlertMeasure::RequestRate,
                AlertMeasureName::UpstreamErrorRatio => AlertMeasure::UpstreamErrorRatio,
                AlertMeasureName::OpenConnections => AlertMeasure::OpenConnections,
            },
            comparison: match self.comparison {
                AlertComparisonName::Above => AlertComparison::Above,
                AlertComparisonName::Below => AlertComparison::Below,
            },
            threshold: self.threshold,
            pending_for: Duration::from_secs(self.pending_seconds),
            site: scope_id("site", self.site, SiteId::new)?,
            route: scope_id("route", self.route, RouteId::new)?,
            upstream: scope_id("upstream", self.upstream, UpstreamPoolId::new)?,
            severity: match self.severity {
                AlertSeverityName::Warning => AlertSeverity::Warning,
                AlertSeverityName::Critical => AlertSeverity::Critical,
            },
            enabled: self.enabled,
            channels: self.channels,
        })
    }
}

impl From<AlertRuleSpec> for AlertRuleSpecBody {
    fn from(value: AlertRuleSpec) -> Self {
        Self {
            name: value.name,
            description: value.description,
            measure: match value.measure {
                AlertMeasure::ServerErrorRatio => AlertMeasureName::ServerErrorRatio,
                AlertMeasure::LatencyP95 => AlertMeasureName::LatencyP95,
                AlertMeasure::RequestRate => AlertMeasureName::RequestRate,
                AlertMeasure::UpstreamErrorRatio => AlertMeasureName::UpstreamErrorRatio,
                AlertMeasure::OpenConnections => AlertMeasureName::OpenConnections,
            },
            comparison: match value.comparison {
                AlertComparison::Above => AlertComparisonName::Above,
                AlertComparison::Below => AlertComparisonName::Below,
            },
            threshold: value.threshold,
            pending_seconds: value.pending_for.as_secs(),
            site: value.site.map(|site| site.to_string()),
            route: value.route.map(|route| route.to_string()),
            upstream: value.upstream.map(|upstream| upstream.to_string()),
            severity: match value.severity {
                AlertSeverity::Warning => AlertSeverityName::Warning,
                AlertSeverity::Critical => AlertSeverityName::Critical,
            },
            enabled: value.enabled,
            channels: value.channels,
        }
    }
}

/// A rule with where it stands.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AlertRuleView {
    pub id: String,
    pub spec: AlertRuleSpecBody,
    pub version: u64,
    /// For `If-Match` when replacing or deleting it.
    pub etag: String,
    pub created_at: String,
    pub updated_at: String,
    pub state: AlertStateName,
    /// When the state began, RFC 3339; absent while inactive.
    pub since: Option<String>,
    /// The measure at the last evaluation; absent without data.
    pub value: Option<f64>,
    /// When the rule was last evaluated; absent before the first time.
    pub evaluated_at: Option<String>,
    /// Why the last evaluation failed; absent when it did not.
    pub evaluation_error: Option<String>,
}

impl From<AlertRule> for AlertRuleView {
    fn from(value: AlertRule) -> Self {
        Self {
            etag: etag(value.version),
            id: value.id,
            spec: value.spec.into(),
            version: value.version,
            created_at: rfc3339(value.created_at),
            updated_at: rfc3339(value.updated_at),
            state: match value.state {
                AlertState::Pending => AlertStateName::Pending,
                AlertState::Firing => AlertStateName::Firing,
                _ => AlertStateName::Inactive,
            },
            since: value.since.map(rfc3339),
            value: value.value,
            evaluated_at: value.evaluated_at.map(rfc3339),
            evaluation_error: Some(value.evaluation_error).filter(|error| !error.is_empty()),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AlertChannelKindName {
    /// Alertmanager's webhook payload, signed as Standard Webhooks specify.
    Webhook,
    /// Reserved: refused as unsupported until the panel can send mail.
    Email,
}

/// A channel; where it sends is shown only as its origin.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AlertChannelView {
    pub id: String,
    pub kind: AlertChannelKindName,
    /// A webhook URL's scheme, host and port.
    pub target: String,
    pub version: u64,
    /// For `If-Match` when rotating or deleting it.
    pub etag: String,
    pub created_at: String,
    pub updated_at: String,
}

impl From<AlertChannel> for AlertChannelView {
    fn from(value: AlertChannel) -> Self {
        Self {
            etag: etag(value.version),
            id: value.id,
            kind: match value.kind {
                AlertChannelKind::Webhook => AlertChannelKindName::Webhook,
                AlertChannelKind::Email => AlertChannelKindName::Email,
            },
            target: value.target,
            version: value.version,
            created_at: rfc3339(value.created_at),
            updated_at: rfc3339(value.updated_at),
        }
    }
}

fn webhook() -> AlertChannelKindName {
    AlertChannelKindName::Webhook
}

/// A channel to create.
#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct NewAlertChannelBody {
    pub id: String,
    #[serde(default = "webhook")]
    pub kind: AlertChannelKindName,
    /// Where notifications are posted, `http` or `https`.
    #[schema(value_type = String)]
    pub url: Zeroizing<String>,
}

/// A new signing secret, and a new URL when one is given.
#[derive(Default, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AlertChannelRotationBody {
    #[serde(default)]
    #[schema(value_type = Option<String>)]
    pub url: Option<Zeroizing<String>>,
}

/// A channel with the secret receivers verify notifications with, shown
/// only now.
#[derive(Serialize, ToSchema)]
pub struct AlertChannelSecretView {
    pub channel: AlertChannelView,
    /// `whsec_` and the base64 key, as Standard Webhooks libraries take it.
    pub secret: String,
}

impl From<AlertChannelSecret> for AlertChannelSecretView {
    fn from(value: AlertChannelSecret) -> Self {
        Self {
            channel: value.channel.into(),
            secret: value.secret.to_string(),
        }
    }
}

/// How a test notification fared.
#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AlertTestView {
    pub delivered: bool,
    /// The receiver's HTTP status, if it answered.
    pub status: Option<u16>,
    /// Why it was not delivered; absent when it was.
    pub failure: Option<String>,
}

impl From<AlertTest> for AlertTestView {
    fn from(value: AlertTest) -> Self {
        Self {
            delivered: value.delivered,
            status: value.status,
            failure: Some(value.failure).filter(|failure| !failure.is_empty()),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AlertNotificationKindName {
    Firing,
    Resolved,
}

#[derive(Clone, Copy, Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AlertNotificationStateName {
    /// Waiting for its first or next attempt.
    Queued,
    Delivered,
    /// Refused by the receiver, or still failing after a day.
    Abandoned,
}

#[derive(Clone, Debug, Serialize, ToSchema)]
pub struct AlertNotificationView {
    /// The `webhook-id` receivers see, the same on every attempt.
    pub id: String,
    pub rule: String,
    pub channel: String,
    pub kind: AlertNotificationKindName,
    pub state: AlertNotificationStateName,
    pub attempts: u32,
    pub created_at: String,
    pub next_attempt_at: Option<String>,
    pub delivered_at: Option<String>,
    /// Why the last attempt failed; absent when none did.
    pub last_failure: Option<String>,
}

impl From<AlertNotification> for AlertNotificationView {
    fn from(value: AlertNotification) -> Self {
        Self {
            id: value.id,
            rule: value.rule,
            channel: value.channel,
            kind: match value.kind {
                AlertNotificationKind::Resolved => AlertNotificationKindName::Resolved,
                _ => AlertNotificationKindName::Firing,
            },
            state: match value.state {
                AlertNotificationState::Delivered => AlertNotificationStateName::Delivered,
                AlertNotificationState::Abandoned => AlertNotificationStateName::Abandoned,
                _ => AlertNotificationStateName::Queued,
            },
            attempts: value.attempts,
            created_at: rfc3339(value.created_at),
            next_attempt_at: value.next_attempt_at.map(rfc3339),
            delivered_at: value.delivered_at.map(rfc3339),
            last_failure: Some(value.last_failure).filter(|failure| !failure.is_empty()),
        }
    }
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Path)]
pub(crate) struct AlertPath {
    id: String,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub(crate) struct NotificationParams {
    /// Only one rule's notifications.
    rule: Option<String>,
    /// Only one channel's notifications.
    channel: Option<String>,
    /// At most 200; 50 by default.
    limit: Option<u32>,
}

/// Every rule with where it stands.
#[utoipa::path(get, path = "/api/v1/alert-rules", params(QueryHeaders),
    responses((status = 200, body = Vec<AlertRuleView>)), tag = "alerts")]
pub(crate) async fn list_alert_rules<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Json<Vec<AlertRuleView>>, ApiError> {
    let rules = port(&state)?.rules(request_scope(&headers)?).await?;
    Ok(Json(rules.into_iter().map(Into::into).collect()))
}

/// Creates a rule, or replaces it when `If-Match` carries its ETag.
#[utoipa::path(put, path = "/api/v1/alert-rules/{id}", request_body = AlertRuleSpecBody,
    params(MutationHeaders, AlertPath,
        ("If-Match" = Option<String>, Header, description = "ETag of the rule being replaced")),
    responses((status = 200, body = AlertRuleView, headers(("ETag" = String))),
        (status = 201, body = AlertRuleView, headers(("ETag" = String)))), tag = "alerts")]
pub(crate) async fn put_alert_rule<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<AlertPath>,
    Json(body): Json<AlertRuleSpecBody>,
) -> Result<Response, ApiError> {
    let context = command_context(&headers)?;
    let version = if_match(&headers)?;
    let rule = port(&state)?
        .put_rule(context, &path.id, body.spec()?, version)
        .await?;
    let status = if version.is_none() {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok(tagged(status, rule.version, AlertRuleView::from(rule)))
}

/// Deletes a rule; a firing alert resolves first. `If-Match` must carry its
/// ETag.
#[utoipa::path(delete, path = "/api/v1/alert-rules/{id}",
    params(MutationHeaders, AlertPath,
        ("If-Match" = String, Header, description = "ETag of the rule")),
    responses((status = 204)), tag = "alerts")]
pub(crate) async fn delete_alert_rule<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<AlertPath>,
) -> Result<StatusCode, ApiError> {
    let context = command_context(&headers)?;
    let version = required_match(&headers, "rule")?;
    port(&state)?
        .delete_rule(context, &path.id, version)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Every channel; where each sends is shown only as its origin.
#[utoipa::path(get, path = "/api/v1/alert-channels", params(QueryHeaders),
    responses((status = 200, body = Vec<AlertChannelView>)), tag = "alerts")]
pub(crate) async fn list_alert_channels<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
) -> Result<Json<Vec<AlertChannelView>>, ApiError> {
    let channels = port(&state)?.channels(request_scope(&headers)?).await?;
    Ok(Json(channels.into_iter().map(Into::into).collect()))
}

fn secret_response(status: StatusCode, secret: AlertChannelSecret) -> Response {
    let version = secret.channel.version;
    let mut response = tagged(status, version, AlertChannelSecretView::from(secret));
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

/// Creates a channel and returns its signing secret, shown only now.
#[utoipa::path(post, path = "/api/v1/alert-channels", request_body = NewAlertChannelBody,
    params(MutationHeaders),
    responses((status = 201, body = AlertChannelSecretView, headers(("ETag" = String)))),
    tag = "alerts")]
pub(crate) async fn create_alert_channel<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Json(body): Json<NewAlertChannelBody>,
) -> Result<Response, ApiError> {
    let context = command_context(&headers)?;
    let channel = NewAlertChannel {
        id: body.id,
        kind: match body.kind {
            AlertChannelKindName::Webhook => AlertChannelKind::Webhook,
            AlertChannelKindName::Email => AlertChannelKind::Email,
        },
        url: body.url,
    };
    let secret = port(&state)?.create_channel(context, channel).await?;
    Ok(secret_response(StatusCode::CREATED, secret))
}

/// Replaces a channel's signing secret, and its URL when one is given, and
/// returns the new secret, shown only now. `If-Match` must carry its ETag.
#[utoipa::path(post, path = "/api/v1/alert-channels/{id}/rotate",
    request_body = AlertChannelRotationBody,
    params(MutationHeaders, AlertPath,
        ("If-Match" = String, Header, description = "ETag of the channel")),
    responses((status = 200, body = AlertChannelSecretView, headers(("ETag" = String)))),
    tag = "alerts")]
pub(crate) async fn rotate_alert_channel<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<AlertPath>,
    body: Option<Json<AlertChannelRotationBody>>,
) -> Result<Response, ApiError> {
    let context = command_context(&headers)?;
    let version = required_match(&headers, "channel")?;
    let url = body.and_then(|Json(body)| body.url);
    let secret = port(&state)?
        .rotate_channel(context, &path.id, url, version)
        .await?;
    Ok(secret_response(StatusCode::OK, secret))
}

/// Deletes a channel no rule names. `If-Match` must carry its ETag.
#[utoipa::path(delete, path = "/api/v1/alert-channels/{id}",
    params(MutationHeaders, AlertPath,
        ("If-Match" = String, Header, description = "ETag of the channel")),
    responses((status = 204)), tag = "alerts")]
pub(crate) async fn delete_alert_channel<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<AlertPath>,
) -> Result<StatusCode, ApiError> {
    let context = command_context(&headers)?;
    let version = required_match(&headers, "channel")?;
    port(&state)?
        .delete_channel(context, &path.id, version)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Sends one test notification now and reports how the receiver answered.
#[utoipa::path(post, path = "/api/v1/alert-channels/{id}/test",
    params(MutationHeaders, AlertPath),
    responses((status = 200, body = AlertTestView)), tag = "alerts")]
pub(crate) async fn test_alert_channel<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Path(path): Path<AlertPath>,
) -> Result<Json<AlertTestView>, ApiError> {
    let context = command_context(&headers)?;
    let tested = port(&state)?.test_channel(context, &path.id).await?;
    Ok(Json(tested.into()))
}

/// Notifications newest first.
#[utoipa::path(get, path = "/api/v1/alert-notifications", params(QueryHeaders, NotificationParams),
    responses((status = 200, body = Vec<AlertNotificationView>)), tag = "alerts")]
pub(crate) async fn list_alert_notifications<U>(
    State(state): State<ApiState<U>>,
    headers: HeaderMap,
    Query(params): Query<NotificationParams>,
) -> Result<Json<Vec<AlertNotificationView>>, ApiError> {
    let query = AlertNotificationQuery {
        rule: params.rule.filter(|rule| !rule.is_empty()),
        channel: params.channel.filter(|channel| !channel.is_empty()),
        limit: params.limit,
    };
    let notifications = port(&state)?
        .notifications(request_scope(&headers)?, query)
        .await?;
    Ok(Json(notifications.into_iter().map(Into::into).collect()))
}
