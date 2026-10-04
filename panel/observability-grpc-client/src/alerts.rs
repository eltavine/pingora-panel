//! `AlertsPort` over `observability-service` (ADR 0027).

use crate::{time, ObservabilityClient};
use async_trait::async_trait;
use panel_application::{
    AlertChannel, AlertChannelKind, AlertChannelSecret, AlertComparison, AlertMeasure,
    AlertNotification, AlertNotificationKind, AlertNotificationQuery, AlertNotificationState,
    AlertRule, AlertRuleSpec, AlertSeverity, AlertState, AlertTest, AlertsPort, CommandContext,
    NewAlertChannel, RequestScope, RouteId, SiteId, UpstreamPoolId,
};
use panel_contracts::observability::v1::{self as wire, alerts_client::AlertsClient};
use panel_errors::{PanelError, Result};
use panel_service::command_context;
use panel_service::{request_context, response_error, status_error};
use std::time::{Duration, UNIX_EPOCH};
use zeroize::Zeroizing;

fn unreadable(what: &str) -> PanelError {
    PanelError::internal(format!("observability-service sent an unreadable {what}"))
}

fn wire_spec(spec: AlertRuleSpec) -> wire::AlertRuleSpec {
    wire::AlertRuleSpec {
        name: spec.name,
        description: spec.description,
        measure: match spec.measure {
            AlertMeasure::ServerErrorRatio => wire::AlertMeasure::ServerErrorRatio,
            AlertMeasure::LatencyP95 => wire::AlertMeasure::LatencyP95,
            AlertMeasure::RequestRate => wire::AlertMeasure::RequestRate,
            AlertMeasure::UpstreamErrorRatio => wire::AlertMeasure::UpstreamErrorRatio,
            AlertMeasure::OpenConnections => wire::AlertMeasure::OpenConnections,
        }
        .into(),
        comparison: match spec.comparison {
            AlertComparison::Above => wire::AlertComparison::Above,
            AlertComparison::Below => wire::AlertComparison::Below,
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
            AlertSeverity::Warning => wire::AlertSeverity::Warning,
            AlertSeverity::Critical => wire::AlertSeverity::Critical,
        }
        .into(),
        enabled: spec.enabled,
        channels: spec.channels,
    }
}

fn optional<T, E>(
    value: String,
    parse: impl FnOnce(String) -> std::result::Result<T, E>,
) -> Result<Option<T>> {
    if value.is_empty() {
        return Ok(None);
    }
    parse(value).map(Some).map_err(|_| unreadable("rule scope"))
}

fn spec(value: wire::AlertRuleSpec) -> Result<AlertRuleSpec> {
    Ok(AlertRuleSpec {
        measure: match wire::AlertMeasure::try_from(value.measure) {
            Ok(wire::AlertMeasure::ServerErrorRatio) => AlertMeasure::ServerErrorRatio,
            Ok(wire::AlertMeasure::LatencyP95) => AlertMeasure::LatencyP95,
            Ok(wire::AlertMeasure::RequestRate) => AlertMeasure::RequestRate,
            Ok(wire::AlertMeasure::UpstreamErrorRatio) => AlertMeasure::UpstreamErrorRatio,
            Ok(wire::AlertMeasure::OpenConnections) => AlertMeasure::OpenConnections,
            _ => return Err(unreadable("measure")),
        },
        comparison: match wire::AlertComparison::try_from(value.comparison) {
            Ok(wire::AlertComparison::Below) => AlertComparison::Below,
            Ok(wire::AlertComparison::Above) => AlertComparison::Above,
            _ => return Err(unreadable("comparison")),
        },
        severity: match wire::AlertSeverity::try_from(value.severity) {
            Ok(wire::AlertSeverity::Warning) => AlertSeverity::Warning,
            Ok(wire::AlertSeverity::Critical) => AlertSeverity::Critical,
            _ => return Err(unreadable("severity")),
        },
        pending_for: value
            .pending_for
            .map(Duration::try_from)
            .transpose()
            .map_err(|_| unreadable("pending period"))?
            .unwrap_or_default(),
        site: optional(value.site, SiteId::new)?,
        route: optional(value.route, RouteId::new)?,
        upstream: optional(value.upstream, UpstreamPoolId::new)?,
        name: value.name,
        description: value.description,
        threshold: value.threshold,
        enabled: value.enabled,
        channels: value.channels,
    })
}

fn rule(value: wire::AlertRule) -> Result<AlertRule> {
    Ok(AlertRule {
        spec: spec(value.spec.ok_or_else(|| unreadable("rule"))?)?,
        state: match wire::AlertState::try_from(value.state) {
            Ok(wire::AlertState::Pending) => AlertState::Pending,
            Ok(wire::AlertState::Firing) => AlertState::Firing,
            _ => AlertState::Inactive,
        },
        id: value.id,
        version: value.version,
        created_at: time(value.created_at).unwrap_or(UNIX_EPOCH),
        updated_at: time(value.updated_at).unwrap_or(UNIX_EPOCH),
        since: time(value.since),
        value: value.value,
        evaluated_at: time(value.evaluated_at),
        evaluation_error: value.evaluation_error,
    })
}

fn wire_kind(kind: AlertChannelKind) -> wire::AlertChannelKind {
    match kind {
        AlertChannelKind::Webhook => wire::AlertChannelKind::Webhook,
        AlertChannelKind::Email => wire::AlertChannelKind::Email,
    }
}

fn channel(value: wire::AlertChannel) -> AlertChannel {
    AlertChannel {
        kind: match wire::AlertChannelKind::try_from(value.kind) {
            Ok(wire::AlertChannelKind::Email) => AlertChannelKind::Email,
            _ => AlertChannelKind::Webhook,
        },
        id: value.id,
        target: value.target,
        version: value.version,
        created_at: time(value.created_at).unwrap_or(UNIX_EPOCH),
        updated_at: time(value.updated_at).unwrap_or(UNIX_EPOCH),
    }
}

fn secret(channel_: Option<wire::AlertChannel>, secret: String) -> Result<AlertChannelSecret> {
    Ok(AlertChannelSecret {
        channel: channel(channel_.ok_or_else(|| unreadable("channel"))?),
        secret: Zeroizing::new(secret),
    })
}

fn notification(value: wire::AlertNotification) -> AlertNotification {
    AlertNotification {
        kind: match wire::AlertNotificationKind::try_from(value.kind) {
            Ok(wire::AlertNotificationKind::Resolved) => AlertNotificationKind::Resolved,
            _ => AlertNotificationKind::Firing,
        },
        state: match wire::AlertNotificationState::try_from(value.state) {
            Ok(wire::AlertNotificationState::Delivered) => AlertNotificationState::Delivered,
            Ok(wire::AlertNotificationState::Abandoned) => AlertNotificationState::Abandoned,
            _ => AlertNotificationState::Queued,
        },
        id: value.id,
        rule: value.rule,
        channel: value.channel,
        attempts: value.attempts,
        created_at: time(value.created_at).unwrap_or(UNIX_EPOCH),
        next_attempt_at: time(value.next_attempt_at),
        delivered_at: time(value.delivered_at),
        last_failure: value.last_failure,
    }
}

#[async_trait]
impl AlertsPort for ObservabilityClient {
    async fn rules(&self, scope: RequestScope) -> Result<Vec<AlertRule>> {
        let message = wire::AlertsListRulesRequest {
            context: Some(request_context(&scope)),
        };
        let response = AlertsClient::new(self.channel.clone())
            .list_rules(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        response.rules.into_iter().map(rule).collect()
    }

    async fn put_rule(
        &self,
        context: CommandContext,
        id: &str,
        spec: AlertRuleSpec,
        version: Option<u64>,
    ) -> Result<AlertRule> {
        let message = wire::AlertsPutRuleRequest {
            context: Some(command_context(&context)),
            id: id.to_owned(),
            spec: Some(wire_spec(spec)),
            expected_version: version.unwrap_or_default(),
        };
        let response = AlertsClient::new(self.channel.clone())
            .put_rule(self.request(message, &context.scope()))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        rule(response.rule.ok_or_else(|| unreadable("rule"))?)
    }

    async fn delete_rule(&self, context: CommandContext, id: &str, version: u64) -> Result<()> {
        let message = wire::AlertsDeleteRuleRequest {
            context: Some(command_context(&context)),
            id: id.to_owned(),
            expected_version: version,
        };
        let response = AlertsClient::new(self.channel.clone())
            .delete_rule(self.request(message, &context.scope()))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)
    }

    async fn channels(&self, scope: RequestScope) -> Result<Vec<AlertChannel>> {
        let message = wire::AlertsListChannelsRequest {
            context: Some(request_context(&scope)),
        };
        let response = AlertsClient::new(self.channel.clone())
            .list_channels(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(response.channels.into_iter().map(channel).collect())
    }

    async fn create_channel(
        &self,
        context: CommandContext,
        channel_: NewAlertChannel,
    ) -> Result<AlertChannelSecret> {
        let message = wire::AlertsCreateChannelRequest {
            context: Some(command_context(&context)),
            id: channel_.id,
            kind: wire_kind(channel_.kind).into(),
            url: channel_.url.to_string(),
            recipients: Vec::new(),
        };
        let response = AlertsClient::new(self.channel.clone())
            .create_channel(self.request(message, &context.scope()))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        secret(response.channel, response.secret)
    }

    async fn rotate_channel(
        &self,
        context: CommandContext,
        id: &str,
        url: Option<Zeroizing<String>>,
        version: u64,
    ) -> Result<AlertChannelSecret> {
        let message = wire::AlertsRotateChannelRequest {
            context: Some(command_context(&context)),
            id: id.to_owned(),
            url: url.map(|url| url.to_string()).unwrap_or_default(),
            expected_version: version,
        };
        let response = AlertsClient::new(self.channel.clone())
            .rotate_channel(self.request(message, &context.scope()))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        secret(response.channel, response.secret)
    }

    async fn delete_channel(&self, context: CommandContext, id: &str, version: u64) -> Result<()> {
        let message = wire::AlertsDeleteChannelRequest {
            context: Some(command_context(&context)),
            id: id.to_owned(),
            expected_version: version,
        };
        let response = AlertsClient::new(self.channel.clone())
            .delete_channel(self.request(message, &context.scope()))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)
    }

    async fn test_channel(&self, context: CommandContext, id: &str) -> Result<AlertTest> {
        let message = wire::AlertsTestChannelRequest {
            context: Some(command_context(&context)),
            id: id.to_owned(),
        };
        let response = AlertsClient::new(self.channel.clone())
            .test_channel(self.request(message, &context.scope()))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(AlertTest {
            delivered: response.delivered,
            status: u16::try_from(response.status)
                .ok()
                .filter(|status| *status != 0),
            failure: response.failure,
        })
    }

    async fn notifications(
        &self,
        scope: RequestScope,
        query: AlertNotificationQuery,
    ) -> Result<Vec<AlertNotification>> {
        let message = wire::AlertsListNotificationsRequest {
            context: Some(request_context(&scope)),
            rule: query.rule.unwrap_or_default(),
            channel: query.channel.unwrap_or_default(),
            limit: query.limit.unwrap_or_default(),
        };
        let response = AlertsClient::new(self.channel.clone())
            .list_notifications(self.request(message, &scope))
            .await
            .map_err(status_error)?
            .into_inner();
        response_error(response.error)?;
        Ok(response
            .notifications
            .into_iter()
            .map(notification)
            .collect())
    }
}
