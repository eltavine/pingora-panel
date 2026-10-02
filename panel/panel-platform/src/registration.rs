use crate::{ServiceDescriptor, ServiceRegistrar};
use panel_errors::{PanelError, Result};
use std::{future::Future, sync::Arc, time::Duration};

/// How a running instance keeps its registration alive.
///
/// The refresh interval must be well below the registry's expiry so that a
/// live instance never lapses; a failed refresh is retried sooner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegistrationPolicy {
    refresh_interval: Duration,
    retry_interval: Duration,
}

impl Default for RegistrationPolicy {
    fn default() -> Self {
        Self {
            refresh_interval: Duration::from_secs(10),
            retry_interval: Duration::from_secs(2),
        }
    }
}

impl RegistrationPolicy {
    pub fn new(refresh_interval: Duration, retry_interval: Duration) -> Result<Self> {
        if refresh_interval.is_zero() || retry_interval.is_zero() {
            return Err(PanelError::invalid_argument(
                "registration intervals must be non-zero",
            ));
        }
        Ok(Self {
            refresh_interval,
            retry_interval,
        })
    }

    pub fn refresh_interval(self) -> Duration {
        self.refresh_interval
    }

    pub fn retry_interval(self) -> Duration {
        self.retry_interval
    }
}

/// Registers `descriptor` and refreshes it until `shutdown` resolves, then
/// deregisters it. Registry failures are logged and retried; they never stop
/// the service, whose registration simply expires while the registry is down.
pub async fn maintain_registration(
    registrar: Arc<dyn ServiceRegistrar>,
    descriptor: ServiceDescriptor,
    policy: RegistrationPolicy,
    shutdown: impl Future<Output = ()>,
) {
    tokio::pin!(shutdown);
    let mut failing = false;
    loop {
        let delay = match registrar.register(&descriptor).await {
            Ok(()) => {
                if failing {
                    tracing::info!(service = %descriptor.service(), "service registration restored");
                }
                failing = false;
                policy.refresh_interval
            }
            Err(error) => {
                if !failing {
                    tracing::warn!(
                        service = %descriptor.service(),
                        error_code = %error.code,
                        "service registration failed; retrying"
                    );
                }
                failing = true;
                policy.retry_interval
            }
        };
        tokio::select! {
            () = &mut shutdown => break,
            () = tokio::time::sleep(delay) => {}
        }
    }
    if let Err(error) = registrar.deregister(&descriptor).await {
        tracing::warn!(
            service = %descriptor.service(),
            error_code = %error.code,
            "service deregistration failed; the registration will expire"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ServiceName;
    use async_trait::async_trait;
    use chrono::Utc;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Recording {
        calls: Mutex<Vec<&'static str>>,
        failures: Mutex<u32>,
    }

    #[async_trait]
    impl ServiceRegistrar for Recording {
        async fn register(&self, _: &ServiceDescriptor) -> Result<()> {
            self.calls.lock().unwrap().push("register");
            let mut failures = self.failures.lock().unwrap();
            if *failures > 0 {
                *failures -= 1;
                return Err(PanelError::unavailable("registry down"));
            }
            Ok(())
        }

        async fn deregister(&self, _: &ServiceDescriptor) -> Result<()> {
            self.calls.lock().unwrap().push("deregister");
            Ok(())
        }
    }

    #[tokio::test]
    async fn refreshes_retries_and_deregisters_on_shutdown() {
        let registrar = Arc::new(Recording::default());
        *registrar.failures.lock().unwrap() = 2;
        let descriptor = ServiceDescriptor::new(
            ServiceName::new("automation-service").unwrap(),
            "0.1.0",
            Utc::now(),
        );
        let policy =
            RegistrationPolicy::new(Duration::from_millis(5), Duration::from_millis(1)).unwrap();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(maintain_registration(
            Arc::clone(&registrar) as Arc<dyn ServiceRegistrar>,
            descriptor,
            policy,
            async {
                let _ = stopped.await;
            },
        ));
        while registrar.calls.lock().unwrap().len() < 5 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        stop.send(()).unwrap();
        task.await.unwrap();

        let calls = registrar.calls.lock().unwrap().clone();
        assert!(calls[..calls.len() - 1]
            .iter()
            .all(|call| *call == "register"));
        assert_eq!(calls.last(), Some(&"deregister"));
        assert_eq!(*registrar.failures.lock().unwrap(), 0);
    }

    #[test]
    fn intervals_must_be_positive() {
        assert!(RegistrationPolicy::new(Duration::ZERO, Duration::from_secs(1)).is_err());
        assert!(RegistrationPolicy::new(Duration::from_secs(1), Duration::ZERO).is_err());
    }
}
