//! Alert rules in the service schema, with where each stands.

use super::{
    expected,
    model::{identifier, Comparison, Measure, RuleSpec, Severity, State},
    payload::{Kind, Notices, Occurrence},
    publish, refused, Cause,
};
use chrono::{DateTime, Utc};
use panel_domain::{RouteId, SiteId, UpstreamPoolId};
use panel_errors::{PanelError, Result};
use panel_event_contracts::observability::v1 as event;
use panel_postgres::{storage_error, EventLog, ServiceDatabase};
use sqlx::{postgres::PgRow, PgConnection, PgPool, Row};
use std::{sync::Arc, time::Duration};

const AGGREGATE: &str = "alert_rule";

/// A rule as kept, with its state.
#[derive(Clone, Debug, PartialEq)]
pub struct RuleRecord {
    pub id: String,
    pub spec: RuleSpec,
    pub version: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub state: State,
    /// When the condition began to hold; unset while inactive.
    pub active_since: Option<DateTime<Utc>>,
    /// When the alert fired; set only while firing.
    pub fired_at: Option<DateTime<Utc>>,
    /// The measure at the last evaluation; unset without data.
    pub value: Option<f64>,
    pub evaluated_at: Option<DateTime<Utc>>,
    /// Why the last evaluation failed; empty when it did not.
    pub evaluation_error: String,
}

impl RuleRecord {
    pub fn etag(&self) -> String {
        format!("\"{}\"", self.version)
    }
}

pub(crate) fn settings(spec: &RuleSpec) -> event::AlertRuleSettings {
    event::AlertRuleSettings {
        name: spec.name.clone(),
        measure: spec.measure.name().to_owned(),
        comparison: spec.comparison.name().to_owned(),
        threshold: spec.threshold,
        pending_seconds: spec.pending_for.as_secs(),
        site: spec
            .site
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default(),
        route: spec
            .route
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default(),
        upstream: spec
            .upstream
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default(),
        severity: spec.severity.name().to_owned(),
        enabled: spec.enabled,
        channels: spec.channels.clone(),
    }
}

macro_rules! select {
    () => {
        "SELECT r.rule_id, r.name, r.description, r.measure, r.comparison, r.threshold, \
         r.pending_seconds, r.site_id, r.route_id, r.upstream_id, r.severity, r.enabled, \
         r.version, r.created_at, r.updated_at, \
         ARRAY(SELECT c.channel_id FROM alert_rule_channels c WHERE c.rule_id = r.rule_id \
         ORDER BY c.channel_id) AS channels, \
         s.state, s.active_since, s.fired_at, s.value, s.evaluated_at, s.evaluation_error \
         FROM alert_rules r LEFT JOIN alert_states s ON s.rule_id = r.rule_id"
    };
}

fn corrupt(what: &str) -> PanelError {
    PanelError::corrupt_state(format!("a stored alert rule has an invalid {what}"))
}

fn id<T, E>(
    value: Option<String>,
    parse: impl FnOnce(String) -> std::result::Result<T, E>,
) -> Result<Option<T>> {
    value.map(parse).transpose().map_err(|_| corrupt("scope"))
}

fn record(row: &PgRow) -> Result<RuleRecord> {
    let text = |column: &str| row.try_get::<String, _>(column).map_err(storage_error);
    let pending: i32 = row.try_get("pending_seconds").map_err(storage_error)?;
    let version: i64 = row.try_get("version").map_err(storage_error)?;
    let state: Option<String> = row.try_get("state").map_err(storage_error)?;
    Ok(RuleRecord {
        id: text("rule_id")?,
        spec: RuleSpec {
            name: text("name")?,
            description: text("description")?,
            measure: Measure::parse(&text("measure")?).ok_or_else(|| corrupt("measure"))?,
            comparison: Comparison::parse(&text("comparison")?)
                .ok_or_else(|| corrupt("comparison"))?,
            threshold: row.try_get("threshold").map_err(storage_error)?,
            pending_for: Duration::from_secs(
                u64::try_from(pending).map_err(|_| corrupt("pending period"))?,
            ),
            site: id(row.try_get("site_id").map_err(storage_error)?, SiteId::new)?,
            route: id(
                row.try_get("route_id").map_err(storage_error)?,
                RouteId::new,
            )?,
            upstream: id(
                row.try_get("upstream_id").map_err(storage_error)?,
                UpstreamPoolId::new,
            )?,
            severity: Severity::parse(&text("severity")?).ok_or_else(|| corrupt("severity"))?,
            enabled: row.try_get("enabled").map_err(storage_error)?,
            channels: row.try_get("channels").map_err(storage_error)?,
        },
        version: u64::try_from(version).map_err(|_| corrupt("version"))?,
        created_at: row.try_get("created_at").map_err(storage_error)?,
        updated_at: row.try_get("updated_at").map_err(storage_error)?,
        state: match state {
            Some(state) => State::parse(&state).ok_or_else(|| corrupt("state"))?,
            None => State::Inactive,
        },
        active_since: row.try_get("active_since").map_err(storage_error)?,
        fired_at: row.try_get("fired_at").map_err(storage_error)?,
        value: row.try_get("value").map_err(storage_error)?,
        evaluated_at: row.try_get("evaluated_at").map_err(storage_error)?,
        evaluation_error: row
            .try_get::<Option<String>, _>("evaluation_error")
            .map_err(storage_error)?
            .unwrap_or_default(),
    })
}

/// Alert rules in the service schema. Changes write their
/// `observability.alert_rule.*` events in the same transaction.
#[derive(Clone)]
pub struct AlertRules {
    pool: PgPool,
    events: EventLog,
    notices: Arc<Notices>,
}

impl AlertRules {
    pub fn new(database: &ServiceDatabase, events: EventLog, notices: Arc<Notices>) -> Self {
        Self {
            pool: database.pool().clone(),
            events,
            notices,
        }
    }

    pub async fn list(&self) -> Result<Vec<RuleRecord>> {
        sqlx::query(concat!(select!(), " ORDER BY r.rule_id"))
            .fetch_all(&self.pool)
            .await
            .map_err(storage_error)?
            .iter()
            .map(record)
            .collect()
    }

    pub async fn get(&self, id: &str) -> Result<RuleRecord> {
        sqlx::query(concat!(select!(), " WHERE r.rule_id = $1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(storage_error)?
            .map(|row| record(&row))
            .transpose()?
            .ok_or_else(|| PanelError::not_found(format!("there is no alert rule {id}")))
    }

    /// Creates rule `id` when `version` is 0, or replaces it at `version`.
    pub async fn put(
        &self,
        cause: Cause<'_>,
        id: &str,
        spec: RuleSpec,
        version: u64,
    ) -> Result<RuleRecord> {
        let result = async {
            let id = identifier(id, "a rule ID")?;
            spec.validate()?;
            let now = Utc::now();
            let mut transaction = self.pool.begin().await.map_err(storage_error)?;
            let current: Option<i64> =
                sqlx::query_scalar("SELECT version FROM alert_rules WHERE rule_id = $1 FOR UPDATE")
                    .bind(&id)
                    .fetch_optional(&mut *transaction)
                    .await
                    .map_err(storage_error)?;
            let current = current.and_then(|version| u64::try_from(version).ok());
            let next = match (current, expected(version)) {
                (None, None) => 1,
                (None, Some(_)) => {
                    return Err(PanelError::not_found(format!(
                        "there is no alert rule {id}"
                    )))
                }
                (Some(_), None) => {
                    return Err(PanelError::conflict(format!(
                        "alert rule {id} already exists"
                    )))
                }
                (Some(current), Some(expected)) if current != expected => {
                    return Err(PanelError::precondition_failed(format!(
                        "alert rule {id} has changed; it is at version {current}, not {expected}"
                    )))
                }
                (Some(current), Some(_)) => current + 1,
            };
            let known: Vec<String> = sqlx::query_scalar(
                "SELECT channel_id FROM alert_channels WHERE channel_id = ANY($1)",
            )
            .bind(&spec.channels)
            .fetch_all(&mut *transaction)
            .await
            .map_err(storage_error)?;
            if let Some(missing) = spec
                .channels
                .iter()
                .find(|channel| !known.contains(channel))
            {
                return Err(PanelError::invalid_argument(format!(
                    "there is no alert channel {missing}"
                )));
            }
            sqlx::query(
                "INSERT INTO alert_rules (rule_id, name, description, measure, comparison, \
                 threshold, pending_seconds, site_id, route_id, upstream_id, severity, enabled, \
                 version, created_at, updated_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $14) \
                 ON CONFLICT (rule_id) DO UPDATE SET name = $2, description = $3, measure = $4, \
                 comparison = $5, threshold = $6, pending_seconds = $7, site_id = $8, \
                 route_id = $9, upstream_id = $10, severity = $11, enabled = $12, version = $13, \
                 updated_at = $14",
            )
            .bind(&id)
            .bind(&spec.name)
            .bind(&spec.description)
            .bind(spec.measure.name())
            .bind(spec.comparison.name())
            .bind(spec.threshold)
            .bind(i32::try_from(spec.pending_for.as_secs()).unwrap_or(i32::MAX))
            .bind(spec.site.as_ref().map(ToString::to_string))
            .bind(spec.route.as_ref().map(ToString::to_string))
            .bind(spec.upstream.as_ref().map(ToString::to_string))
            .bind(spec.severity.name())
            .bind(spec.enabled)
            .bind(i64::try_from(next).unwrap_or(i64::MAX))
            .bind(now)
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?;
            sqlx::query("DELETE FROM alert_rule_channels WHERE rule_id = $1")
                .bind(&id)
                .execute(&mut *transaction)
                .await
                .map_err(storage_error)?;
            sqlx::query(
                "INSERT INTO alert_rule_channels (rule_id, channel_id) \
                 SELECT $1, channel FROM unnest($2::text[]) AS channel",
            )
            .bind(&id)
            .bind(&spec.channels)
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?;
            let changed = settings(&spec);
            if next == 1 {
                let data = event::AlertRuleCreated {
                    id: id.clone(),
                    settings: Some(changed),
                    version: next,
                };
                publish(
                    &self.events,
                    &mut transaction,
                    cause,
                    (AGGREGATE, &id),
                    &data,
                )
                .await?;
            } else {
                let data = event::AlertRuleUpdated {
                    id: id.clone(),
                    settings: Some(changed),
                    version: next,
                };
                publish(
                    &self.events,
                    &mut transaction,
                    cause,
                    (AGGREGATE, &id),
                    &data,
                )
                .await?;
            }
            transaction.commit().await.map_err(storage_error)?;
            self.get(&id).await
        }
        .await;
        let operation = if version == 0 { "create" } else { "update" };
        refused::<event::AlertRuleRefused, _>(
            &self.events,
            cause,
            (AGGREGATE, id),
            operation,
            result,
        )
        .await
    }

    /// Deletes rule `id`; a firing alert resolves first, so receivers do not
    /// keep it open.
    pub async fn delete(&self, cause: Cause<'_>, id: &str, version: u64) -> Result<()> {
        let result = async {
            let mut transaction = self.pool.begin().await.map_err(storage_error)?;
            let rule = sqlx::query(concat!(select!(), " WHERE r.rule_id = $1 FOR UPDATE OF r"))
                .bind(id)
                .fetch_optional(&mut *transaction)
                .await
                .map_err(storage_error)?
                .map(|row| record(&row))
                .transpose()?
                .ok_or_else(|| PanelError::not_found(format!("there is no alert rule {id}")))?;
            if let Some(version) = expected(version).filter(|version| *version != rule.version) {
                return Err(PanelError::precondition_failed(format!(
                    "alert rule {id} has changed; it is at version {}, not {version}",
                    rule.version
                )));
            }
            if let (State::Firing, Some(fired_at)) = (rule.state, rule.fired_at) {
                self.resolve(&mut transaction, cause, &rule, fired_at, Utc::now())
                    .await?;
            }
            sqlx::query("DELETE FROM alert_rules WHERE rule_id = $1")
                .bind(id)
                .execute(&mut *transaction)
                .await
                .map_err(storage_error)?;
            publish(
                &self.events,
                &mut transaction,
                cause,
                (AGGREGATE, id),
                &event::AlertRuleDeleted { id: id.to_owned() },
            )
            .await?;
            transaction.commit().await.map_err(storage_error)
        }
        .await;
        refused::<event::AlertRuleRefused, _>(
            &self.events,
            cause,
            (AGGREGATE, id),
            "delete",
            result,
        )
        .await
    }

    /// Queues the resolution of `rule`'s alert, fired at `fired_at`, and
    /// records it.
    pub(crate) async fn resolve(
        &self,
        connection: &mut PgConnection,
        cause: Cause<'_>,
        rule: &RuleRecord,
        fired_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let occurrence = Occurrence {
            kind: Kind::Resolved,
            fired_at,
            resolved_at: Some(now),
            value: rule.value,
        };
        self.notices
            .queue(connection, rule, &occurrence, now)
            .await?;
        publish(
            &self.events,
            connection,
            cause,
            ("alert", &rule.id),
            &event::AlertResolved {
                rule: rule.id.clone(),
                name: rule.spec.name.clone(),
                fired_at: Some(fired_at.into()),
            },
        )
        .await
    }

    /// Queues the notifications of `rule`'s alert firing and records it.
    pub(crate) async fn fire(
        &self,
        connection: &mut PgConnection,
        cause: Cause<'_>,
        rule: &RuleRecord,
        since: DateTime<Utc>,
        value: f64,
        now: DateTime<Utc>,
    ) -> Result<()> {
        let occurrence = Occurrence {
            kind: Kind::Firing,
            fired_at: now,
            resolved_at: None,
            value: Some(value),
        };
        self.notices
            .queue(connection, rule, &occurrence, now)
            .await?;
        publish(
            &self.events,
            connection,
            cause,
            ("alert", &rule.id),
            &event::AlertFired {
                rule: rule.id.clone(),
                name: rule.spec.name.clone(),
                severity: rule.spec.severity.name().to_owned(),
                value,
                threshold: rule.spec.threshold,
                since: Some(since.into()),
            },
        )
        .await
    }

    /// Records where `rule` stands after an evaluation at `now`.
    pub(crate) async fn store_state(
        &self,
        connection: &mut PgConnection,
        rule: &RuleRecord,
        now: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO alert_states (rule_id, state, active_since, fired_at, value, \
             evaluated_at, evaluation_error) VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (rule_id) DO UPDATE SET state = $2, active_since = $3, fired_at = $4, \
             value = $5, evaluated_at = $6, evaluation_error = $7",
        )
        .bind(&rule.id)
        .bind(rule.state.name())
        .bind(rule.active_since)
        .bind(rule.fired_at)
        .bind(rule.value)
        .bind(now)
        .bind(&rule.evaluation_error)
        .execute(connection)
        .await
        .map_err(storage_error)?;
        Ok(())
    }

    pub(crate) fn pool(&self) -> &PgPool {
        &self.pool
    }
}
