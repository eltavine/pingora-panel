//! The `Alerts` gRPC service over rules, channels and notifications.

use super::{
    channels::{AlertChannels, ChannelKind, ChannelRecord},
    model::{Comparison, Measure, RuleSpec, Severity, State},
    notifier::{NotificationRecord, Notifier},
    rules::{AlertRules, RuleRecord},
    Cause,
};
use chrono::{DateTime, Utc};
use panel_contracts::{
    common::v1 as common,
    observability::v1::{self as wire, alerts_server},
};
use panel_domain::{RouteId, SiteId, UpstreamPoolId};
use panel_errors::{PanelError, Result};
use panel_events::{RequestId, RequestScope};
use panel_postgres::EventLog;
use std::time::{Duration, SystemTime};
use tonic::{Request, Response, Status};

fn timestamp(time: DateTime<Utc>) -> prost_types::Timestamp {
    SystemTime::from(time).into()
}

/// The request's scope and who made it.
fn caller(context: Option<common::RequestContext>) -> Result<(RequestScope, String)> {
    let context =
        context.ok_or_else(|| PanelError::invalid_argument("request context is required"))?;
    let scope = RequestScope::new(RequestId::new(context.request_id)?);
    let scope = if context.correlation_id.is_empty() {
        scope
    } else {
        scope.with_correlation_id(RequestId::new(context.correlation_id)?)
    };
    Ok((scope, context.actor))
}

fn optional<T, E: std::fmt::Display>(
    value: String,
    what: &str,
    parse: impl FnOnce(String) -> std::result::Result<T, E>,
) -> Result<Option<T>> {
    if value.is_empty() {
        return Ok(None);
    }
    parse(value)
        .map(Some)
        .map_err(|error| PanelError::invalid_argument(format!("{what}: {error}")))
}

fn spec(value: Option<wire::AlertRuleSpec>) -> Result<RuleSpec> {
    let value = value.ok_or_else(|| PanelError::invalid_argument("a rule is required"))?;
    let measure = match wire::AlertMeasure::try_from(value.measure) {
        Ok(wire::AlertMeasure::ServerErrorRatio) => Measure::ServerErrorRatio,
        Ok(wire::AlertMeasure::LatencyP95) => Measure::LatencyP95,
        Ok(wire::AlertMeasure::RequestRate) => Measure::RequestRate,
        Ok(wire::AlertMeasure::UpstreamErrorRatio) => Measure::UpstreamErrorRatio,
        Ok(wire::AlertMeasure::OpenConnections) => Measure::OpenConnections,
        _ => return Err(PanelError::invalid_argument("a rule names its measure")),
    };
    let comparison = match wire::AlertComparison::try_from(value.comparison) {
        Ok(wire::AlertComparison::Above) => Comparison::Above,
        Ok(wire::AlertComparison::Below) => Comparison::Below,
        _ => {
            return Err(PanelError::invalid_argument(
                "a rule compares above or below",
            ))
        }
    };
    let severity = match wire::AlertSeverity::try_from(value.severity) {
        Ok(wire::AlertSeverity::Warning) => Severity::Warning,
        Ok(wire::AlertSeverity::Critical) => Severity::Critical,
        _ => return Err(PanelError::invalid_argument("a rule names its severity")),
    };
    let pending_for = match value.pending_for {
        None => Duration::ZERO,
        Some(duration) => Duration::try_from(duration)
            .map_err(|_| PanelError::invalid_argument("the pending period is not negative"))?,
    };
    Ok(RuleSpec {
        name: value.name,
        description: value.description,
        measure,
        comparison,
        threshold: value.threshold,
        pending_for,
        site: optional(value.site, "site", SiteId::new)?,
        route: optional(value.route, "route", RouteId::new)?,
        upstream: optional(value.upstream, "upstream", UpstreamPoolId::new)?,
        severity,
        enabled: value.enabled,
        channels: value.channels,
    })
}

fn rule(value: RuleRecord) -> wire::AlertRule {
    let spec = value.spec;
    wire::AlertRule {
        id: value.id,
        spec: Some(wire::AlertRuleSpec {
            name: spec.name,
            description: spec.description,
            measure: match spec.measure {
                Measure::ServerErrorRatio => wire::AlertMeasure::ServerErrorRatio,
                Measure::LatencyP95 => wire::AlertMeasure::LatencyP95,
                Measure::RequestRate => wire::AlertMeasure::RequestRate,
                Measure::UpstreamErrorRatio => wire::AlertMeasure::UpstreamErrorRatio,
                Measure::OpenConnections => wire::AlertMeasure::OpenConnections,
            }
            .into(),
            comparison: match spec.comparison {
                Comparison::Above => wire::AlertComparison::Above,
                Comparison::Below => wire::AlertComparison::Below,
            }
            .into(),
            threshold: spec.threshold,
            pending_for: prost_types::Duration::try_from(spec.pending_for).ok(),
            site: spec.site.map(|site| site.to_string()).unwrap_or_default(),
            route: spec
                .route
                .map(|route| route.to_string())
                .unwrap_or_default(),
            upstream: spec
                .upstream
                .map(|upstream| upstream.to_string())
                .unwrap_or_default(),
            severity: match spec.severity {
                Severity::Warning => wire::AlertSeverity::Warning,
                Severity::Critical => wire::AlertSeverity::Critical,
            }
            .into(),
            enabled: spec.enabled,
            channels: spec.channels,
        }),
        version: value.version,
        created_at: Some(timestamp(value.created_at)),
        updated_at: Some(timestamp(value.updated_at)),
        state: match value.state {
            State::Inactive => wire::AlertState::Inactive,
            State::Pending => wire::AlertState::Pending,
            State::Firing => wire::AlertState::Firing,
        }
        .into(),
        since: match value.state {
            State::Firing => value.fired_at,
            _ => value.active_since,
        }
        .map(timestamp),
        value: value.value,
        evaluated_at: value.evaluated_at.map(timestamp),
        evaluation_error: value.evaluation_error,
    }
}

fn channel(value: ChannelRecord) -> wire::AlertChannel {
    wire::AlertChannel {
        id: value.id,
        kind: match value.kind {
            ChannelKind::Webhook => wire::AlertChannelKind::Webhook,
            ChannelKind::Email => wire::AlertChannelKind::Email,
        }
        .into(),
        target: value.target,
        version: value.version,
        created_at: Some(timestamp(value.created_at)),
        updated_at: Some(timestamp(value.updated_at)),
    }
}

fn notification(value: NotificationRecord) -> wire::AlertNotification {
    wire::AlertNotification {
        id: value.id.to_string(),
        rule: value.rule,
        channel: value.channel,
        kind: match value.kind.as_str() {
            "firing" => wire::AlertNotificationKind::Firing,
            "resolved" => wire::AlertNotificationKind::Resolved,
            _ => wire::AlertNotificationKind::Unspecified,
        }
        .into(),
        state: match value.state.as_str() {
            "queued" => wire::AlertNotificationState::Queued,
            "delivered" => wire::AlertNotificationState::Delivered,
            "abandoned" => wire::AlertNotificationState::Abandoned,
            _ => wire::AlertNotificationState::Unspecified,
        }
        .into(),
        attempts: value.attempts,
        created_at: Some(timestamp(value.created_at)),
        next_attempt_at: value.next_attempt_at.map(timestamp),
        delivered_at: value.delivered_at.map(timestamp),
        last_failure: value.last_failure,
    }
}

/// Serves `pingora.panel.observability.v1.Alerts`.
#[derive(Clone)]
pub struct AlertsService {
    rules: AlertRules,
    channels: AlertChannels,
    notifier: Notifier,
}

impl AlertsService {
    pub fn new(rules: AlertRules, channels: AlertChannels, notifier: Notifier) -> Self {
        Self {
            rules,
            channels,
            notifier,
        }
    }
}

macro_rules! changed_by {
    ($context:expr, |$cause:ident| $body:expr) => {{
        async {
            let (scope, actor) = caller($context)?;
            let principal = EventLog::user(&actor);
            let $cause = Cause {
                scope: &scope,
                principal: &principal,
            };
            $body.await
        }
        .await
    }};
}

#[tonic::async_trait]
impl alerts_server::Alerts for AlertsService {
    async fn list_rules(
        &self,
        request: Request<wire::AlertsListRulesRequest>,
    ) -> std::result::Result<Response<wire::AlertsListRulesResponse>, Status> {
        let result = async {
            caller(request.into_inner().context)?;
            self.rules.list().await
        }
        .await;
        Ok(Response::new(match result {
            Ok(rules) => wire::AlertsListRulesResponse {
                rules: rules.into_iter().map(rule).collect(),
                error: None,
            },
            Err(error) => wire::AlertsListRulesResponse {
                rules: Vec::new(),
                error: Some(error.into()),
            },
        }))
    }

    async fn put_rule(
        &self,
        request: Request<wire::AlertsPutRuleRequest>,
    ) -> std::result::Result<Response<wire::AlertsPutRuleResponse>, Status> {
        let request = request.into_inner();
        let result: Result<RuleRecord> = changed_by!(request.context, |cause| async {
            let spec = spec(request.spec)?;
            self.rules
                .put(cause, &request.id, spec, request.expected_version)
                .await
        });
        Ok(Response::new(match result {
            Ok(changed) => wire::AlertsPutRuleResponse {
                rule: Some(rule(changed)),
                error: None,
            },
            Err(error) => wire::AlertsPutRuleResponse {
                rule: None,
                error: Some(error.into()),
            },
        }))
    }

    async fn delete_rule(
        &self,
        request: Request<wire::AlertsDeleteRuleRequest>,
    ) -> std::result::Result<Response<wire::AlertsDeleteRuleResponse>, Status> {
        let request = request.into_inner();
        let result: Result<()> = changed_by!(request.context, |cause| self.rules.delete(
            cause,
            &request.id,
            request.expected_version
        ));
        Ok(Response::new(wire::AlertsDeleteRuleResponse {
            error: result.err().map(Into::into),
        }))
    }

    async fn list_channels(
        &self,
        request: Request<wire::AlertsListChannelsRequest>,
    ) -> std::result::Result<Response<wire::AlertsListChannelsResponse>, Status> {
        let result = async {
            caller(request.into_inner().context)?;
            self.channels.list().await
        }
        .await;
        Ok(Response::new(match result {
            Ok(channels) => wire::AlertsListChannelsResponse {
                channels: channels.into_iter().map(channel).collect(),
                error: None,
            },
            Err(error) => wire::AlertsListChannelsResponse {
                channels: Vec::new(),
                error: Some(error.into()),
            },
        }))
    }

    async fn create_channel(
        &self,
        request: Request<wire::AlertsCreateChannelRequest>,
    ) -> std::result::Result<Response<wire::AlertsCreateChannelResponse>, Status> {
        let request = request.into_inner();
        let kind = match wire::AlertChannelKind::try_from(request.kind) {
            Ok(wire::AlertChannelKind::Email) => ChannelKind::Email,
            _ => ChannelKind::Webhook,
        };
        let result = changed_by!(request.context, |cause| self.channels.create(
            cause,
            &request.id,
            kind,
            &request.url
        ));
        Ok(Response::new(match result {
            Ok((created, secret)) => wire::AlertsCreateChannelResponse {
                channel: Some(channel(created)),
                secret: secret.to_string(),
                error: None,
            },
            Err(error) => wire::AlertsCreateChannelResponse {
                error: Some(error.into()),
                ..wire::AlertsCreateChannelResponse::default()
            },
        }))
    }

    async fn rotate_channel(
        &self,
        request: Request<wire::AlertsRotateChannelRequest>,
    ) -> std::result::Result<Response<wire::AlertsRotateChannelResponse>, Status> {
        let request = request.into_inner();
        let url = (!request.url.is_empty()).then_some(request.url.as_str());
        let result = changed_by!(request.context, |cause| self.channels.rotate(
            cause,
            &request.id,
            url,
            request.expected_version
        ));
        Ok(Response::new(match result {
            Ok((rotated, secret)) => wire::AlertsRotateChannelResponse {
                channel: Some(channel(rotated)),
                secret: secret.to_string(),
                error: None,
            },
            Err(error) => wire::AlertsRotateChannelResponse {
                error: Some(error.into()),
                ..wire::AlertsRotateChannelResponse::default()
            },
        }))
    }

    async fn delete_channel(
        &self,
        request: Request<wire::AlertsDeleteChannelRequest>,
    ) -> std::result::Result<Response<wire::AlertsDeleteChannelResponse>, Status> {
        let request = request.into_inner();
        let result: Result<()> = changed_by!(request.context, |cause| self.channels.delete(
            cause,
            &request.id,
            request.expected_version
        ));
        Ok(Response::new(wire::AlertsDeleteChannelResponse {
            error: result.err().map(Into::into),
        }))
    }

    async fn test_channel(
        &self,
        request: Request<wire::AlertsTestChannelRequest>,
    ) -> std::result::Result<Response<wire::AlertsTestChannelResponse>, Status> {
        let request = request.into_inner();
        let result = async {
            caller(request.context)?;
            self.notifier.test(&request.id).await
        }
        .await;
        Ok(Response::new(match result {
            Ok(outcome) => wire::AlertsTestChannelResponse {
                delivered: outcome.delivered,
                status: outcome.status.map(u32::from).unwrap_or_default(),
                failure: outcome.failure,
                error: None,
            },
            Err(error) => wire::AlertsTestChannelResponse {
                error: Some(error.into()),
                ..wire::AlertsTestChannelResponse::default()
            },
        }))
    }

    async fn list_notifications(
        &self,
        request: Request<wire::AlertsListNotificationsRequest>,
    ) -> std::result::Result<Response<wire::AlertsListNotificationsResponse>, Status> {
        let request = request.into_inner();
        let result = async {
            caller(request.context)?;
            let filter = |value: &str| (!value.is_empty()).then_some(value.to_owned());
            let (rule, channel) = (filter(&request.rule), filter(&request.channel));
            self.notifier
                .list(
                    rule.as_deref(),
                    channel.as_deref(),
                    (request.limit != 0).then_some(request.limit),
                )
                .await
        }
        .await;
        Ok(Response::new(match result {
            Ok(notifications) => wire::AlertsListNotificationsResponse {
                notifications: notifications.into_iter().map(notification).collect(),
                error: None,
            },
            Err(error) => wire::AlertsListNotificationsResponse {
                notifications: Vec::new(),
                error: Some(error.into()),
            },
        }))
    }
}
