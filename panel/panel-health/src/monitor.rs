use crate::{HealthRegistry, HealthReport, HealthStatus, ServiceMode};
use std::{sync::Arc, time::Duration};
use tokio::{sync::watch, task::JoinHandle};

/// How soon an unhealthy service is evaluated again, so that startup and
/// recovery are reported promptly.
const UNHEALTHY_RECHECK: Duration = Duration::from_millis(500);

/// Re-evaluates a registry and publishes each report: at the given interval
/// while healthy, and more often while any check fails or warns.
pub struct HealthMonitor {
    registry: Arc<HealthRegistry>,
    interval: Duration,
}

impl HealthMonitor {
    pub fn new(registry: Arc<HealthRegistry>, interval: Duration) -> Self {
        Self { registry, interval }
    }

    /// Starts evaluating immediately. The task ends once every
    /// [`HealthWatch`] is dropped.
    pub fn spawn(self) -> (HealthWatch, JoinHandle<()>) {
        let (sender, receiver) = watch::channel(HealthReport::starting(self.registry.identity()));
        let task = tokio::spawn(async move {
            loop {
                let report = tokio::select! {
                    report = self.registry.evaluate() => report,
                    () = sender.closed() => return,
                };
                let delay = if report.status() == HealthStatus::Pass {
                    self.interval
                } else {
                    self.interval.min(UNHEALTHY_RECHECK)
                };
                sender.send_replace(report);
                tokio::select! {
                    () = tokio::time::sleep(delay) => {}
                    () = sender.closed() => return,
                }
            }
        });
        (HealthWatch { receiver }, task)
    }
}

/// The latest published health of a service.
#[derive(Clone, Debug)]
pub struct HealthWatch {
    receiver: watch::Receiver<HealthReport>,
}

impl HealthWatch {
    /// A watch that always reports `report`, for compositions without
    /// dependency checks.
    pub fn fixed(report: HealthReport) -> Self {
        let (sender, receiver) = watch::channel(report);
        drop(sender);
        Self { receiver }
    }

    pub fn current(&self) -> HealthReport {
        self.receiver.borrow().clone()
    }

    pub fn mode(&self) -> ServiceMode {
        self.receiver.borrow().mode()
    }

    /// Waits for the next published report; `false` once the monitor stopped.
    pub async fn changed(&mut self) -> bool {
        self.receiver.changed().await.is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CheckOutcome, ComponentType, HealthCheck, HealthStatus, Impact, ServiceIdentity};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct Switch(AtomicBool);

    #[async_trait]
    impl HealthCheck for Switch {
        fn component(&self) -> &str {
            "postgresql"
        }
        fn component_type(&self) -> ComponentType {
            ComponentType::Datastore
        }
        async fn check(&self) -> CheckOutcome {
            if self.0.load(Ordering::SeqCst) {
                CheckOutcome::pass()
            } else {
                CheckOutcome::fail("unreachable")
            }
        }
    }

    #[tokio::test]
    async fn publishes_starting_then_each_evaluation() {
        let switch = Arc::new(Switch(AtomicBool::new(true)));
        let registry = HealthRegistry::new(ServiceIdentity::new("panel-api", "0.1.0")).register(
            Arc::clone(&switch) as Arc<dyn HealthCheck>,
            Impact::Degrading,
        );
        let (sender, receiver) = watch::channel(HealthReport::starting(registry.identity()));
        drop(sender);
        assert_eq!(
            HealthWatch { receiver }.mode(),
            ServiceMode::Unavailable,
            "nothing is served before the first evaluation"
        );

        let (mut watch, task) =
            HealthMonitor::new(Arc::new(registry), Duration::from_millis(10)).spawn();
        while watch.current().status() != HealthStatus::Pass {
            assert!(watch.changed().await);
        }
        assert_eq!(watch.mode(), ServiceMode::Normal);

        switch.0.store(false, Ordering::SeqCst);
        while watch.mode() != ServiceMode::Degraded {
            assert!(watch.changed().await);
        }

        drop(watch);
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("monitor stops when unobserved")
            .unwrap();
    }

    #[tokio::test]
    async fn unhealthy_services_are_rechecked_sooner_than_the_interval() {
        let switch = Arc::new(Switch(AtomicBool::new(false)));
        let registry = HealthRegistry::new(ServiceIdentity::new("panel-api", "0.1.0")).register(
            Arc::clone(&switch) as Arc<dyn HealthCheck>,
            Impact::Required,
        );
        let (mut watch, task) =
            HealthMonitor::new(Arc::new(registry), Duration::from_secs(3600)).spawn();
        while !watch
            .current()
            .checks()
            .contains_key("postgresql:responseTime")
        {
            assert!(watch.changed().await);
        }
        assert_eq!(watch.mode(), ServiceMode::Unavailable);

        switch.0.store(true, Ordering::SeqCst);
        tokio::time::timeout(Duration::from_secs(5), async {
            while watch.mode() != ServiceMode::Normal {
                assert!(watch.changed().await);
            }
        })
        .await
        .expect("recovery is noticed without waiting for the full interval");
        drop(watch);
        task.await.unwrap();
    }

    #[test]
    fn fixed_watches_report_their_document() {
        let identity = ServiceIdentity::new("panel-api", "0.1.0");
        let watch = HealthWatch::fixed(HealthReport::alive(&identity));
        assert_eq!(watch.mode(), ServiceMode::Normal);
        assert_eq!(watch.current().service_id(), "panel-api");
    }
}
