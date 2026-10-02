use crate::{HealthRegistry, HealthReport, ServiceMode};
use std::{sync::Arc, time::Duration};
use tokio::{sync::watch, task::JoinHandle, time::MissedTickBehavior};

/// Re-evaluates a registry on a fixed interval and publishes each report.
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
            let mut ticker = tokio::time::interval(self.interval);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = ticker.tick() => {}
                    () = sender.closed() => return,
                }
                let report = tokio::select! {
                    report = self.registry.evaluate() => report,
                    () = sender.closed() => return,
                };
                sender.send_replace(report);
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

    #[test]
    fn fixed_watches_report_their_document() {
        let identity = ServiceIdentity::new("panel-api", "0.1.0");
        let watch = HealthWatch::fixed(HealthReport::alive(&identity));
        assert_eq!(watch.mode(), ServiceMode::Normal);
        assert_eq!(watch.current().service_id(), "panel-api");
    }
}
