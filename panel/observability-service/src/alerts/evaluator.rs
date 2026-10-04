//! Evaluates alert rules every 30 seconds on the instance that holds the
//! schema's alert lock (ADR 0027).

use super::{
    measures,
    model::State,
    rules::{AlertRules, RuleRecord},
    Cause,
};
use chrono::{DateTime, TimeDelta, Utc};
use panel_errors::{PanelError, Result};
use panel_events::{Actor, Principal, RequestId, RequestScope};
use panel_postgres::storage_error;
use prometheus_http_query::Client;
use sqlx::{
    pool::PoolConnection,
    postgres::{PgAdvisoryLock, PgAdvisoryLockGuard},
    Either, Postgres,
};
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// How often rules are evaluated.
pub const INTERVAL: Duration = Duration::from_secs(30);
const LOCK: &str = "pingora-panel observability alert evaluation";

/// What an evaluation decided for one rule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Transition {
    Unchanged,
    Fired,
    Resolved,
}

/// Where a rule stands after reading `value` at `now`; the alert fires once
/// the condition has held for the pending period and resolves as soon as it
/// does not hold.
pub(crate) fn next(
    rule: &RuleRecord,
    value: Option<f64>,
    now: DateTime<Utc>,
) -> (RuleRecord, Transition) {
    let spec = &rule.spec;
    let holds = value.is_some_and(|value| spec.comparison.holds(value, spec.threshold));
    let mut next = rule.clone();
    next.value = value;
    next.evaluated_at = Some(now);
    next.evaluation_error = String::new();
    if !holds {
        next.state = State::Inactive;
        next.active_since = None;
        next.fired_at = None;
        let transition = if rule.state == State::Firing {
            Transition::Resolved
        } else {
            Transition::Unchanged
        };
        return (next, transition);
    }
    let since = rule.active_since.unwrap_or(now);
    next.active_since = Some(since);
    let pending = TimeDelta::from_std(spec.pending_for).unwrap_or(TimeDelta::MAX);
    if rule.state == State::Firing {
        return (next, Transition::Unchanged);
    }
    if now - since >= pending {
        next.state = State::Firing;
        next.fired_at = Some(now);
        (next, Transition::Fired)
    } else {
        next.state = State::Pending;
        (next, Transition::Unchanged)
    }
}

/// Evaluates rules and records their alerts.
#[derive(Clone)]
pub struct Evaluator {
    rules: AlertRules,
    prometheus: Arc<Client>,
    principal: Principal,
}

impl Evaluator {
    pub fn new(rules: AlertRules, prometheus: Client, service: &str) -> Result<Self> {
        Ok(Self {
            rules,
            prometheus: Arc::new(prometheus),
            principal: Principal::system(
                Actor::new(service).map_err(|error| PanelError::internal(error.to_string()))?,
            ),
        })
    }

    async fn try_lead(&self) -> Result<Option<PgAdvisoryLockGuard<PoolConnection<Postgres>>>> {
        let connection = self.rules.pool().acquire().await.map_err(storage_error)?;
        Ok(
            match PgAdvisoryLock::new(LOCK)
                .try_acquire(connection)
                .await
                .map_err(storage_error)?
            {
                Either::Left(guard) => Some(guard),
                Either::Right(_) => None,
            },
        )
    }

    /// Evaluates rules every 30 seconds while this instance leads, until
    /// `shutdown`; `migrated` resolves once the schema is ready.
    pub async fn run(
        self,
        migrated: impl std::future::Future<Output = bool>,
        shutdown: CancellationToken,
    ) {
        tokio::select! {
            () = shutdown.cancelled() => return,
            ready = migrated => if !ready { return },
        }
        let mut leadership = None;
        let mut ticker = tokio::time::interval(INTERVAL);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                _ = ticker.tick() => {}
            }
            if let Some(guard) = leadership.as_mut() {
                let guard: &mut PgAdvisoryLockGuard<PoolConnection<Postgres>> = guard;
                if sqlx::query("SELECT 1")
                    .execute(guard.as_mut())
                    .await
                    .is_err()
                {
                    tracing::warn!("alert evaluation lost its lock; standing by");
                    leadership = None;
                }
            }
            if leadership.is_none() {
                match self.try_lead().await {
                    Ok(Some(guard)) => {
                        tracing::info!("evaluating alert rules");
                        leadership = Some(guard);
                    }
                    Ok(None) => continue,
                    Err(error) => {
                        tracing::warn!(error = %error.message, "cannot take the alert lock");
                        continue;
                    }
                }
            }
            if let Err(error) = self.evaluate(Utc::now()).await {
                tracing::warn!(error = %error.message, "alert evaluation failed");
            }
        }
        if let Some(guard) = leadership {
            let _ = guard.release_now().await;
        }
    }

    /// Evaluates every rule once at `now`.
    pub async fn evaluate(&self, now: DateTime<Utc>) -> Result<()> {
        let scope = RequestScope::new(
            RequestId::new(Uuid::now_v7().to_string())
                .map_err(|error| PanelError::internal(error.to_string()))?,
        );
        let cause = Cause {
            scope: &scope,
            principal: &self.principal,
        };
        for rule in self.rules.list().await? {
            if let Err(error) = self.evaluate_rule(cause, &rule, now).await {
                tracing::warn!(rule = %rule.id, error = %error.message, "cannot evaluate an alert rule");
            }
        }
        Ok(())
    }

    async fn evaluate_rule(
        &self,
        cause: Cause<'_>,
        rule: &RuleRecord,
        now: DateTime<Utc>,
    ) -> Result<()> {
        if !rule.spec.enabled {
            if rule.state == State::Inactive {
                return Ok(());
            }
            let mut transaction = self.rules.pool().begin().await.map_err(storage_error)?;
            if let (State::Firing, Some(fired_at)) = (rule.state, rule.fired_at) {
                self.rules
                    .resolve(&mut transaction, cause, rule, fired_at, now)
                    .await?;
            }
            let mut idle = rule.clone();
            idle.state = State::Inactive;
            idle.active_since = None;
            idle.fired_at = None;
            self.rules.store_state(&mut transaction, &idle, now).await?;
            return transaction.commit().await.map_err(storage_error);
        }
        let (next, transition) = match measures::read(&self.prometheus, &rule.spec).await {
            Ok(value) => next(rule, value, now),
            Err(error) => {
                let mut kept = rule.clone();
                kept.evaluation_error = error.message;
                (kept, Transition::Unchanged)
            }
        };
        let mut transaction = self.rules.pool().begin().await.map_err(storage_error)?;
        match transition {
            Transition::Fired => {
                let since = next.active_since.unwrap_or(now);
                let value = next.value.unwrap_or_default();
                self.rules
                    .fire(&mut transaction, cause, &next, since, value, now)
                    .await?;
            }
            Transition::Resolved => {
                let fired_at = rule.fired_at.unwrap_or(now);
                self.rules
                    .resolve(&mut transaction, cause, &next, fired_at, now)
                    .await?;
            }
            Transition::Unchanged => {}
        }
        self.rules.store_state(&mut transaction, &next, now).await?;
        transaction.commit().await.map_err(storage_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alerts::model::{Comparison, Measure, RuleSpec, Severity};

    fn rule(pending: u64) -> RuleRecord {
        RuleRecord {
            id: "errors".into(),
            spec: RuleSpec {
                name: "Errors".into(),
                description: String::new(),
                measure: Measure::ServerErrorRatio,
                comparison: Comparison::Above,
                threshold: 0.1,
                pending_for: Duration::from_secs(pending),
                site: None,
                route: None,
                upstream: None,
                severity: Severity::Critical,
                enabled: true,
                channels: Vec::new(),
            },
            version: 1,
            created_at: DateTime::UNIX_EPOCH,
            updated_at: DateTime::UNIX_EPOCH,
            state: State::Inactive,
            active_since: None,
            fired_at: None,
            value: None,
            evaluated_at: None,
            evaluation_error: String::new(),
        }
    }

    #[test]
    fn alerts_fire_after_the_pending_period_and_resolve_at_once() {
        let start = DateTime::UNIX_EPOCH + TimeDelta::hours(1);
        let (pending, transition) = next(&rule(60), Some(0.5), start);
        assert_eq!(
            (pending.state, transition),
            (State::Pending, Transition::Unchanged)
        );
        assert_eq!(pending.active_since, Some(start));

        let later = start + TimeDelta::seconds(30);
        let (still, transition) = next(&pending, Some(0.5), later);
        assert_eq!(
            (still.state, transition),
            (State::Pending, Transition::Unchanged)
        );

        let due = start + TimeDelta::seconds(60);
        let (firing, transition) = next(&still, Some(0.5), due);
        assert_eq!(
            (firing.state, transition),
            (State::Firing, Transition::Fired)
        );
        assert_eq!(firing.fired_at, Some(due));

        let (kept, transition) = next(&firing, Some(0.2), due + TimeDelta::seconds(30));
        assert_eq!(
            (kept.state, transition),
            (State::Firing, Transition::Unchanged)
        );
        assert_eq!(kept.fired_at, Some(due));

        let (resolved, transition) = next(&kept, Some(0.05), due + TimeDelta::seconds(60));
        assert_eq!(
            (resolved.state, transition),
            (State::Inactive, Transition::Resolved)
        );
        assert_eq!((resolved.active_since, resolved.fired_at), (None, None));
    }

    #[test]
    fn rules_without_a_pending_period_fire_at_once_and_no_data_never_fires() {
        let now = DateTime::UNIX_EPOCH;
        assert_eq!(next(&rule(0), Some(0.5), now).1, Transition::Fired);
        let (idle, transition) = next(&rule(0), None, now);
        assert_eq!(
            (idle.state, transition),
            (State::Inactive, Transition::Unchanged)
        );
    }
}
