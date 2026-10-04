//! Alert rules, the channels they notify and the notifications sent
//! (ADR 0027), as the API serves them. `observability-service` owns them
//! and records their changes in the audit trail itself.

use crate::{CommandContext, RequestScope};
use async_trait::async_trait;
use panel_domain::{RouteId, SiteId, UpstreamPoolId};
use panel_errors::Result;
use std::time::{Duration, SystemTime};
use zeroize::Zeroizing;

/// What a rule reads, over the last five minutes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AlertMeasure {
    /// The share of requests answered with a 5xx status.
    ServerErrorRatio,
    /// The 95th percentile request latency, in seconds.
    LatencyP95,
    RequestRate,
    UpstreamErrorRatio,
    OpenConnections,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AlertComparison {
    Above,
    Below,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AlertSeverity {
    Warning,
    Critical,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AlertState {
    Inactive,
    Pending,
    Firing,
}

/// What a rule watches and whom it tells.
#[derive(Clone, Debug, PartialEq)]
pub struct AlertRuleSpec {
    pub name: String,
    pub description: String,
    pub measure: AlertMeasure,
    pub comparison: AlertComparison,
    pub threshold: f64,
    pub pending_for: Duration,
    pub site: Option<SiteId>,
    pub route: Option<RouteId>,
    pub upstream: Option<UpstreamPoolId>,
    pub severity: AlertSeverity,
    pub enabled: bool,
    pub channels: Vec<String>,
}

/// A rule as kept, with where it stands.
#[derive(Clone, Debug, PartialEq)]
pub struct AlertRule {
    pub id: String,
    pub spec: AlertRuleSpec,
    pub version: u64,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub state: AlertState,
    /// When the state began; unset while inactive.
    pub since: Option<SystemTime>,
    pub value: Option<f64>,
    pub evaluated_at: Option<SystemTime>,
    /// Why the last evaluation failed; empty when it did not.
    pub evaluation_error: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AlertChannelKind {
    Webhook,
    /// Reserved: refused until the panel can send mail.
    Email,
}

/// A channel; where it sends is shown only as its origin.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlertChannel {
    pub id: String,
    pub kind: AlertChannelKind,
    pub target: String,
    pub version: u64,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

pub struct NewAlertChannel {
    pub id: String,
    pub kind: AlertChannelKind,
    /// A webhook's URL, which can authorize whoever holds it.
    pub url: Zeroizing<String>,
}

/// A channel with the signing secret receivers verify notifications with,
/// returned only when the channel is created or rotated.
pub struct AlertChannelSecret {
    pub channel: AlertChannel,
    pub secret: Zeroizing<String>,
}

/// How a test notification fared.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlertTest {
    pub delivered: bool,
    pub status: Option<u16>,
    pub failure: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AlertNotificationKind {
    Firing,
    Resolved,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum AlertNotificationState {
    Queued,
    Delivered,
    Abandoned,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AlertNotification {
    pub id: String,
    pub rule: String,
    pub channel: String,
    pub kind: AlertNotificationKind,
    pub state: AlertNotificationState,
    pub attempts: u32,
    pub created_at: SystemTime,
    pub next_attempt_at: Option<SystemTime>,
    pub delivered_at: Option<SystemTime>,
    pub last_failure: String,
}

/// Which notifications to list; unset fields match every one.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AlertNotificationQuery {
    pub rule: Option<String>,
    pub channel: Option<String>,
    /// At most 200; 50 by default.
    pub limit: Option<u32>,
}

#[async_trait]
pub trait AlertsPort: Send + Sync {
    /// Every rule with its state.
    async fn rules(&self, scope: RequestScope) -> Result<Vec<AlertRule>>;

    /// Creates rule `id` without `version`, or replaces it at `version`.
    async fn put_rule(
        &self,
        context: CommandContext,
        id: &str,
        spec: AlertRuleSpec,
        version: Option<u64>,
    ) -> Result<AlertRule>;

    async fn delete_rule(&self, context: CommandContext, id: &str, version: u64) -> Result<()>;

    async fn channels(&self, scope: RequestScope) -> Result<Vec<AlertChannel>>;

    async fn create_channel(
        &self,
        context: CommandContext,
        channel: NewAlertChannel,
    ) -> Result<AlertChannelSecret>;

    /// Replaces the signing secret, and the URL when one is given.
    async fn rotate_channel(
        &self,
        context: CommandContext,
        id: &str,
        url: Option<Zeroizing<String>>,
        version: u64,
    ) -> Result<AlertChannelSecret>;

    async fn delete_channel(&self, context: CommandContext, id: &str, version: u64) -> Result<()>;

    async fn test_channel(&self, context: CommandContext, id: &str) -> Result<AlertTest>;

    /// Newest first.
    async fn notifications(
        &self,
        scope: RequestScope,
        query: AlertNotificationQuery,
    ) -> Result<Vec<AlertNotification>>;
}
