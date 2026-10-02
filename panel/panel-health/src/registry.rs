use crate::{CheckOutcome, ComponentHealth, HealthCheck, HealthReport, Impact};
use chrono::Utc;
use futures_util::{future::join_all, FutureExt};
use std::{panic::AssertUnwindSafe, sync::Arc, time::Duration};
use tokio::time::Instant;

/// Upper bound for one check unless registered with another timeout.
pub const DEFAULT_CHECK_TIMEOUT: Duration = Duration::from_secs(2);

/// Names the service in its health documents.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServiceIdentity {
    service_id: String,
    release_id: String,
    version: String,
}

impl ServiceIdentity {
    /// `service_id` names the service; `release_id` is its build version.
    pub fn new(service_id: impl Into<String>, release_id: impl Into<String>) -> Self {
        Self {
            service_id: service_id.into(),
            release_id: release_id.into(),
            version: "1".into(),
        }
    }

    /// The public interface version, `1` by default.
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    pub fn service_id(&self) -> &str {
        &self.service_id
    }

    pub fn release_id(&self) -> &str {
        &self.release_id
    }

    pub fn version(&self) -> &str {
        &self.version
    }
}

struct Registered {
    check: Arc<dyn HealthCheck>,
    impact: Impact,
    timeout: Duration,
}

/// The checks of one service and the impact of each.
pub struct HealthRegistry {
    identity: ServiceIdentity,
    checks: Vec<Registered>,
}

impl HealthRegistry {
    pub fn new(identity: ServiceIdentity) -> Self {
        Self {
            identity,
            checks: Vec::new(),
        }
    }

    pub fn register(self, check: Arc<dyn HealthCheck>, impact: Impact) -> Self {
        self.register_with_timeout(check, impact, DEFAULT_CHECK_TIMEOUT)
    }

    pub fn register_with_timeout(
        mut self,
        check: Arc<dyn HealthCheck>,
        impact: Impact,
        timeout: Duration,
    ) -> Self {
        self.checks.push(Registered {
            check,
            impact,
            timeout,
        });
        self
    }

    pub fn identity(&self) -> &ServiceIdentity {
        &self.identity
    }

    /// Runs every check concurrently. A check that times out or panics fails
    /// without affecting the others.
    pub async fn evaluate(&self) -> HealthReport {
        let components = join_all(self.checks.iter().map(|registered| async move {
            let started = Instant::now();
            let outcome = tokio::time::timeout(
                registered.timeout,
                AssertUnwindSafe(registered.check.check()).catch_unwind(),
            )
            .await
            .unwrap_or_else(|_| Ok(CheckOutcome::fail("check timed out")))
            .unwrap_or_else(|_| CheckOutcome::fail("check failed unexpectedly"));
            let health = ComponentHealth::new(
                registered.check.component_type(),
                started.elapsed(),
                outcome.status(),
                outcome.output().map(Into::into),
                Utc::now(),
                registered.impact,
            );
            (registered.check.component().to_owned(), health)
        }))
        .await;
        HealthReport::aggregate(&self.identity, components)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ComponentType, HealthStatus, ServiceMode};
    use async_trait::async_trait;

    struct Fixed {
        name: &'static str,
        outcome: CheckOutcome,
    }

    #[async_trait]
    impl HealthCheck for Fixed {
        fn component(&self) -> &str {
            self.name
        }
        fn component_type(&self) -> ComponentType {
            ComponentType::Datastore
        }
        async fn check(&self) -> CheckOutcome {
            self.outcome.clone()
        }
    }

    struct Stalled;

    #[async_trait]
    impl HealthCheck for Stalled {
        fn component(&self) -> &str {
            "stalled"
        }
        fn component_type(&self) -> ComponentType {
            ComponentType::Component
        }
        async fn check(&self) -> CheckOutcome {
            tokio::time::sleep(Duration::from_secs(60)).await;
            CheckOutcome::pass()
        }
    }

    struct Panicking;

    #[async_trait]
    impl HealthCheck for Panicking {
        fn component(&self) -> &str {
            "panicking"
        }
        fn component_type(&self) -> ComponentType {
            ComponentType::Component
        }
        async fn check(&self) -> CheckOutcome {
            panic!("injected check panic")
        }
    }

    fn fixed(name: &'static str, outcome: CheckOutcome) -> Arc<dyn HealthCheck> {
        Arc::new(Fixed { name, outcome })
    }

    fn identity() -> ServiceIdentity {
        ServiceIdentity::new("config-service", "0.1.0")
    }

    async fn evaluate(checks: Vec<(Arc<dyn HealthCheck>, Impact)>) -> HealthReport {
        checks
            .into_iter()
            .fold(
                HealthRegistry::new(identity()),
                |registry, (check, impact)| registry.register(check, impact),
            )
            .evaluate()
            .await
    }

    #[tokio::test]
    async fn impact_decides_between_failing_and_degrading() {
        let healthy = evaluate(vec![(
            fixed("postgresql", CheckOutcome::pass()),
            Impact::Required,
        )])
        .await;
        assert_eq!(
            (healthy.status(), healthy.mode()),
            (HealthStatus::Pass, ServiceMode::Normal)
        );
        assert!(healthy.output().is_none());

        let down = CheckOutcome::fail("connection refused");
        let unavailable = evaluate(vec![
            (fixed("postgresql", down.clone()), Impact::Required),
            (fixed("nats", down.clone()), Impact::Degrading),
        ])
        .await;
        assert_eq!(
            (unavailable.status(), unavailable.mode()),
            (HealthStatus::Fail, ServiceMode::Unavailable)
        );
        assert_eq!(unavailable.http_status(), 503);

        let degraded = evaluate(vec![
            (fixed("postgresql", down.clone()), Impact::Degrading),
            (fixed("gateway", CheckOutcome::pass()), Impact::Required),
        ])
        .await;
        assert_eq!(
            (degraded.status(), degraded.mode()),
            (HealthStatus::Warn, ServiceMode::Degraded)
        );
        assert_eq!(degraded.http_status(), 200);
        assert!(degraded.mode().accepts_reads() && !degraded.mode().accepts_writes());

        let informational = evaluate(vec![
            (fixed("otel", down), Impact::Informational),
            (
                fixed("disk", CheckOutcome::warn("80% used")),
                Impact::Required,
            ),
        ])
        .await;
        assert_eq!(
            (informational.status(), informational.mode()),
            (HealthStatus::Warn, ServiceMode::Normal)
        );
    }

    #[tokio::test]
    async fn stalled_or_panicking_checks_fail_alone() {
        let registry = HealthRegistry::new(identity())
            .register_with_timeout(
                Arc::new(Stalled),
                Impact::Degrading,
                Duration::from_millis(20),
            )
            .register(Arc::new(Panicking), Impact::Informational)
            .register(fixed("postgresql", CheckOutcome::pass()), Impact::Required);
        let report = registry.evaluate().await;
        let output = |key: &str| report.checks()[key][0].output().map(str::to_owned);
        assert_eq!(
            output("stalled:responseTime").as_deref(),
            Some("check timed out")
        );
        assert_eq!(
            output("panicking:responseTime").as_deref(),
            Some("check failed unexpectedly")
        );
        assert_eq!(
            report.checks()["postgresql:responseTime"][0].status(),
            HealthStatus::Pass
        );
        assert_eq!(report.mode(), ServiceMode::Degraded);
    }

    #[tokio::test]
    async fn documents_follow_the_health_response_format() {
        let report = evaluate(vec![(
            fixed("postgresql", CheckOutcome::fail("connection refused")),
            Impact::Degrading,
        )])
        .await;
        let document = serde_json::to_value(&report).unwrap();
        assert_eq!(document["status"], "warn");
        assert_eq!(document["version"], "1");
        assert_eq!(document["releaseId"], "0.1.0");
        assert_eq!(document["serviceId"], "config-service");
        let check = &document["checks"]["postgresql:responseTime"][0];
        assert_eq!(check["componentType"], "datastore");
        assert_eq!(check["observedUnit"], "ms");
        assert_eq!(check["status"], "fail");
        assert_eq!(check["output"], "connection refused");
        assert!(chrono::DateTime::parse_from_rfc3339(check["time"].as_str().unwrap()).is_ok());
        assert!(check.get("impact").is_none());

        let alive = serde_json::to_value(HealthReport::alive(&identity())).unwrap();
        assert_eq!(alive["status"], "pass");
        assert!(alive.get("checks").is_none() && alive.get("output").is_none());
    }
}
