use crate::failure_latch::BackgroundTaskFailureLatch;
use futures_util::{stream::FuturesUnordered, FutureExt, StreamExt};
use panel_errors::{PanelError, Result as PanelResult};
use std::{
    any::Any,
    collections::BTreeMap,
    future::Future,
    panic::AssertUnwindSafe,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::watch, task::JoinHandle};

pub const DEFAULT_BACKGROUND_TASK_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);
pub const MAX_BACKGROUND_TASK_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BackgroundTaskShutdownPolicy {
    total_timeout: Duration,
}

impl BackgroundTaskShutdownPolicy {
    pub fn new(total_timeout: Duration) -> PanelResult<Self> {
        if total_timeout.is_zero() {
            return Err(PanelError::invalid_argument(
                "background task shutdown timeout must be non-zero",
            ));
        }
        if total_timeout > MAX_BACKGROUND_TASK_SHUTDOWN_TIMEOUT {
            return Err(PanelError::invalid_argument(format!(
                "background task shutdown timeout must not exceed {} milliseconds",
                MAX_BACKGROUND_TASK_SHUTDOWN_TIMEOUT.as_millis()
            )));
        }
        Ok(Self { total_timeout })
    }

    pub fn total_timeout(self) -> Duration {
        self.total_timeout
    }
}

impl Default for BackgroundTaskShutdownPolicy {
    fn default() -> Self {
        Self {
            total_timeout: DEFAULT_BACKGROUND_TASK_SHUTDOWN_TIMEOUT,
        }
    }
}

#[derive(Clone)]
pub struct BackgroundTaskSupervisor {
    inner: Arc<BackgroundTaskSupervisorInner>,
}

struct BackgroundTaskSupervisorInner {
    shutdown: watch::Sender<bool>,
    failure: BackgroundTaskFailureLatch,
    registry: Mutex<BackgroundTaskRegistry>,
}

#[derive(Default)]
struct BackgroundTaskRegistry {
    shutting_down: bool,
    tasks: Vec<ManagedBackgroundTask>,
}

struct ManagedBackgroundTask {
    name: String,
    handle: JoinHandle<std::result::Result<(), BackgroundTaskFailure>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum BackgroundTaskFailureKind {
    Panicked,
    Cancelled,
    UnexpectedExit,
    TimedOut,
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct BackgroundTaskFailure {
    task_name: String,
    kind: BackgroundTaskFailureKind,
    detail: String,
}

impl BackgroundTaskFailure {
    pub fn task_name(&self) -> &str {
        &self.task_name
    }

    pub fn kind(&self) -> BackgroundTaskFailureKind {
        self.kind
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }

    fn panicked(task_name: String, panic: Box<dyn Any + Send>) -> Self {
        let detail = panic
            .downcast_ref::<&str>()
            .map(|value| (*value).to_owned())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "task panicked with a non-string payload".into());
        Self {
            task_name,
            kind: BackgroundTaskFailureKind::Panicked,
            detail,
        }
    }

    fn cancelled(task_name: String, detail: String) -> Self {
        Self {
            task_name,
            kind: BackgroundTaskFailureKind::Cancelled,
            detail,
        }
    }

    fn unexpected_exit(task_name: String) -> Self {
        Self {
            task_name,
            kind: BackgroundTaskFailureKind::UnexpectedExit,
            detail: "critical task exited before shutdown was requested".into(),
        }
    }

    fn timed_out(task_names: &[String], timeout: Duration) -> Self {
        let task_name = task_names
            .first()
            .cloned()
            .unwrap_or_else(|| "background-task-supervisor".into());
        let pending = if task_names.is_empty() {
            "unknown".into()
        } else {
            task_names.join(", ")
        };
        Self {
            task_name,
            kind: BackgroundTaskFailureKind::TimedOut,
            detail: format!(
                "shutdown exceeded {} milliseconds; pending tasks: {pending}",
                timeout.as_millis()
            ),
        }
    }
}

enum ManagedTaskCompletion {
    Expected,
    UnexpectedExit,
}

#[derive(Debug, thiserror::Error)]
#[error(
    "background task `{}` failed ({:?}): {}",
    failure.task_name,
    failure.kind,
    failure.detail
)]
pub struct BackgroundTaskError {
    failure: BackgroundTaskFailure,
}

impl BackgroundTaskError {
    pub fn failure(&self) -> &BackgroundTaskFailure {
        &self.failure
    }

    pub fn from_failure(failure: BackgroundTaskFailure) -> Self {
        Self { failure }
    }
}

#[derive(Clone)]
pub struct BackgroundTaskShutdown {
    receiver: watch::Receiver<bool>,
}

impl BackgroundTaskShutdown {
    pub fn is_requested(&self) -> bool {
        *self.receiver.borrow()
    }

    pub async fn requested(mut self) {
        if self.is_requested() {
            return;
        }
        let _ = self.receiver.changed().await;
    }
}

pub struct BackgroundTaskFailureMonitor {
    receiver: watch::Receiver<Option<BackgroundTaskFailure>>,
}

impl BackgroundTaskFailureMonitor {
    pub fn latest(&self) -> Option<BackgroundTaskFailure> {
        self.receiver.borrow().clone()
    }

    pub async fn next(&mut self) -> Option<BackgroundTaskFailure> {
        if let Some(failure) = self.latest() {
            return Some(failure);
        }
        while self.receiver.changed().await.is_ok() {
            if let Some(failure) = self.latest() {
                return Some(failure);
            }
        }
        None
    }
}

impl BackgroundTaskSupervisor {
    pub fn new() -> Self {
        let (shutdown, _) = watch::channel(false);
        Self {
            inner: Arc::new(BackgroundTaskSupervisorInner {
                shutdown,
                failure: BackgroundTaskFailureLatch::new(),
                registry: Mutex::new(BackgroundTaskRegistry::default()),
            }),
        }
    }

    pub fn spawn(&self, name: impl Into<String>, task: impl Future<Output = ()> + Send + 'static) {
        let shutdown = BackgroundTaskShutdown {
            receiver: self.inner.shutdown.subscribe(),
        };
        self.spawn_managed(name.into(), async move {
            tokio::select! {
                _ = task => ManagedTaskCompletion::Expected,
                _ = shutdown.requested() => ManagedTaskCompletion::Expected,
            }
        });
    }

    pub fn spawn_critical(
        &self,
        name: impl Into<String>,
        task: impl Future<Output = ()> + Send + 'static,
    ) {
        let shutdown = BackgroundTaskShutdown {
            receiver: self.inner.shutdown.subscribe(),
        };
        self.spawn_managed(name.into(), async move {
            tokio::select! {
                _ = task => ManagedTaskCompletion::UnexpectedExit,
                _ = shutdown.requested() => ManagedTaskCompletion::Expected,
            }
        });
    }

    pub fn spawn_cooperative<F, T>(&self, name: impl Into<String>, task: F)
    where
        F: FnOnce(BackgroundTaskShutdown) -> T + Send + 'static,
        T: Future<Output = ()> + Send + 'static,
    {
        let shutdown = BackgroundTaskShutdown {
            receiver: self.inner.shutdown.subscribe(),
        };
        self.spawn_managed(name.into(), async move {
            task(shutdown).await;
            ManagedTaskCompletion::Expected
        });
    }

    pub fn spawn_cooperative_critical<F, T>(&self, name: impl Into<String>, task: F)
    where
        F: FnOnce(BackgroundTaskShutdown) -> T + Send + 'static,
        T: Future<Output = ()> + Send + 'static,
    {
        let shutdown = BackgroundTaskShutdown {
            receiver: self.inner.shutdown.subscribe(),
        };
        let shutdown_observer = shutdown.clone();
        self.spawn_managed(name.into(), async move {
            task(shutdown).await;
            if shutdown_observer.is_requested() {
                ManagedTaskCompletion::Expected
            } else {
                ManagedTaskCompletion::UnexpectedExit
            }
        });
    }

    fn spawn_managed(
        &self,
        name: String,
        task: impl Future<Output = ManagedTaskCompletion> + Send + 'static,
    ) {
        let mut registry = self
            .inner
            .registry
            .lock()
            .expect("background task registry mutex poisoned");
        if registry.shutting_down {
            return;
        }
        let failure_sender = self.inner.failure.clone();
        let task_name = name.clone();
        let handle = tokio::spawn(async move {
            match AssertUnwindSafe(task).catch_unwind().await {
                Ok(ManagedTaskCompletion::Expected) => Ok(()),
                Ok(ManagedTaskCompletion::UnexpectedExit) => {
                    let failure = BackgroundTaskFailure::unexpected_exit(task_name);
                    failure_sender.report(failure.clone());
                    Err(failure)
                }
                Err(panic) => {
                    let failure = BackgroundTaskFailure::panicked(task_name, panic);
                    failure_sender.report(failure.clone());
                    Err(failure)
                }
            }
        });
        registry.tasks.push(ManagedBackgroundTask { name, handle });
    }

    pub fn failure_monitor(&self) -> BackgroundTaskFailureMonitor {
        BackgroundTaskFailureMonitor {
            receiver: self.inner.failure.subscribe(),
        }
    }

    pub fn task_count(&self) -> usize {
        self.inner
            .registry
            .lock()
            .expect("background task registry mutex poisoned")
            .tasks
            .len()
    }

    pub async fn shutdown_and_join(&self) -> Result<(), BackgroundTaskError> {
        let tasks = self.begin_shutdown();
        let mut first_error = None;
        for task in tasks {
            let failure = match task.handle.await {
                Ok(Ok(())) => None,
                Ok(Err(failure)) => Some(failure),
                Err(error) => Some(BackgroundTaskFailure::cancelled(
                    task.name,
                    error.to_string(),
                )),
            };
            if let Some(failure) = failure {
                self.inner.failure.report(failure.clone());
                first_error.get_or_insert_with(|| BackgroundTaskError::from_failure(failure));
            }
        }
        self.inner
            .failure
            .latest()
            .map(BackgroundTaskError::from_failure)
            .or(first_error)
            .map_or(Ok(()), Err)
    }

    /// Cooperatively stops every registered task within one shared time budget.
    ///
    /// The legacy `shutdown_and_join` method remains unbounded for source and
    /// behavior compatibility. Production compositions should select an explicit
    /// policy and use this method so one extension cannot stall process exit.
    pub async fn shutdown_and_join_with_policy(
        &self,
        policy: BackgroundTaskShutdownPolicy,
    ) -> Result<(), BackgroundTaskError> {
        let tasks = self.begin_shutdown();
        let mut pending = FuturesUnordered::new();
        let mut aborts = BTreeMap::new();
        for (id, task) in tasks.into_iter().enumerate() {
            aborts.insert(id, (task.name.clone(), task.handle.abort_handle()));
            pending.push(async move { (id, task.name, task.handle.await) });
        }

        let mut first_error = None;
        let joined = tokio::time::timeout(policy.total_timeout(), async {
            while let Some((id, name, result)) = pending.next().await {
                aborts.remove(&id);
                let failure = match result {
                    Ok(Ok(())) => None,
                    Ok(Err(failure)) => Some(failure),
                    Err(error) => Some(BackgroundTaskFailure::cancelled(name, error.to_string())),
                };
                if let Some(failure) = failure {
                    self.inner.failure.report(failure.clone());
                    first_error.get_or_insert_with(|| BackgroundTaskError::from_failure(failure));
                }
            }
        })
        .await;

        if joined.is_err() {
            let pending_names = aborts
                .values()
                .map(|(name, _)| name.clone())
                .collect::<Vec<_>>();
            for (_, abort) in aborts.values() {
                abort.abort();
            }
            // Dropping the join futures after requesting cancellation is
            // intentional. Tokio cancellation is cooperative: awaiting an
            // aborted task that is currently executing blocking or otherwise
            // non-yielding code would turn this total budget back into an
            // unbounded wait. The detached task will be reaped by Tokio when it
            // next yields or returns.
            drop(pending);
            let failure = BackgroundTaskFailure::timed_out(&pending_names, policy.total_timeout());
            self.inner.failure.report(failure.clone());
            let root_cause = self.inner.failure.latest().unwrap_or(failure);
            return Err(BackgroundTaskError::from_failure(root_cause));
        }

        self.inner
            .failure
            .latest()
            .map(BackgroundTaskError::from_failure)
            .or(first_error)
            .map_or(Ok(()), Err)
    }

    fn begin_shutdown(&self) -> Vec<ManagedBackgroundTask> {
        let mut registry = self
            .inner
            .registry
            .lock()
            .expect("background task registry mutex poisoned");
        registry.shutting_down = true;
        let _ = self.inner.shutdown.send(true);
        std::mem::take(&mut registry.tasks)
    }
}

impl Default for BackgroundTaskSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for BackgroundTaskSupervisorInner {
    fn drop(&mut self) {
        for task in self
            .registry
            .get_mut()
            .expect("background task registry mutex poisoned")
            .tasks
            .drain(..)
        {
            task.handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    #[tokio::test]
    async fn shutdown_cancels_and_joins_registered_tasks() {
        let supervisor = BackgroundTaskSupervisor::new();
        let completed = Arc::new(AtomicBool::new(false));
        let completed_from_task = Arc::clone(&completed);
        supervisor.spawn("pending", async move {
            std::future::pending::<()>().await;
            completed_from_task.store(true, Ordering::Relaxed);
        });

        assert_eq!(supervisor.task_count(), 1);
        supervisor.shutdown_and_join().await.unwrap();
        assert_eq!(supervisor.task_count(), 0);
        assert!(!completed.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn task_panics_are_reported_with_the_task_name() {
        let supervisor = BackgroundTaskSupervisor::new();
        let mut monitor = supervisor.failure_monitor();
        supervisor.spawn("failing-consumer", async { panic!("injected panic") });

        let failure = monitor.next().await.unwrap();
        assert_eq!(failure.task_name(), "failing-consumer");
        assert_eq!(failure.kind(), BackgroundTaskFailureKind::Panicked);
        assert_eq!(failure.detail(), "injected panic");

        let error = supervisor.shutdown_and_join().await.unwrap_err();
        assert!(error.to_string().contains("failing-consumer"));
    }

    #[tokio::test]
    async fn cooperative_tasks_finish_their_shutdown_protocol() {
        let supervisor = BackgroundTaskSupervisor::new();
        let drained = Arc::new(AtomicBool::new(false));
        let drained_from_task = Arc::clone(&drained);
        supervisor.spawn_cooperative("draining-consumer", move |shutdown| async move {
            shutdown.requested().await;
            drained_from_task.store(true, Ordering::Relaxed);
        });

        supervisor.shutdown_and_join().await.unwrap();
        assert!(drained.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn critical_tasks_report_an_unexpected_clean_exit() {
        let supervisor = BackgroundTaskSupervisor::new();
        let mut monitor = supervisor.failure_monitor();
        supervisor.spawn_critical("short-lived-critical-task", async {});

        let failure = monitor.next().await.unwrap();
        assert_eq!(failure.task_name(), "short-lived-critical-task");
        assert_eq!(failure.kind(), BackgroundTaskFailureKind::UnexpectedExit);
        assert!(supervisor.shutdown_and_join().await.is_err());
    }

    #[tokio::test]
    async fn bounded_shutdown_aborts_a_non_cooperative_task() {
        let supervisor = BackgroundTaskSupervisor::new();
        supervisor.spawn_cooperative_critical("stuck-extension", |shutdown| async move {
            shutdown.requested().await;
            std::future::pending::<()>().await;
        });
        let policy = BackgroundTaskShutdownPolicy::new(Duration::from_millis(10)).unwrap();

        let error = supervisor
            .shutdown_and_join_with_policy(policy)
            .await
            .unwrap_err();

        assert_eq!(error.failure().kind(), BackgroundTaskFailureKind::TimedOut);
        assert_eq!(error.failure().task_name(), "stuck-extension");
        assert!(error.failure().detail().contains("stuck-extension"));
        assert_eq!(supervisor.task_count(), 0);
    }

    #[tokio::test]
    async fn shutdown_timeout_cannot_replace_the_failure_that_triggered_shutdown() {
        let supervisor = BackgroundTaskSupervisor::new();
        let mut monitor = supervisor.failure_monitor();
        supervisor.spawn_critical("initiating-failure", async {});
        supervisor.spawn_cooperative_critical("stuck-during-shutdown", |shutdown| async move {
            shutdown.requested().await;
            std::future::pending::<()>().await;
        });
        let initiating_failure = monitor.next().await.unwrap();

        let error = supervisor
            .shutdown_and_join_with_policy(
                BackgroundTaskShutdownPolicy::new(Duration::from_millis(10)).unwrap(),
            )
            .await
            .unwrap_err();

        assert_eq!(error.failure(), &initiating_failure);
        assert_eq!(error.failure().task_name(), "initiating-failure");
        assert_eq!(
            error.failure().kind(),
            BackgroundTaskFailureKind::UnexpectedExit
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_timeout_is_a_real_total_budget_for_non_yielding_code() {
        let supervisor = BackgroundTaskSupervisor::new();
        supervisor.spawn_cooperative_critical("blocking-extension", |shutdown| async move {
            shutdown.requested().await;
            std::thread::sleep(Duration::from_millis(400));
        });
        let policy = BackgroundTaskShutdownPolicy::new(Duration::from_millis(20)).unwrap();
        let started = Instant::now();

        let error = supervisor
            .shutdown_and_join_with_policy(policy)
            .await
            .unwrap_err();

        assert_eq!(error.failure().kind(), BackgroundTaskFailureKind::TimedOut);
        assert!(
            started.elapsed() < Duration::from_millis(150),
            "shutdown exceeded its total budget: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn shutdown_budget_is_non_zero_and_bounded() {
        assert!(BackgroundTaskShutdownPolicy::new(Duration::ZERO).is_err());
        assert!(BackgroundTaskShutdownPolicy::new(MAX_BACKGROUND_TASK_SHUTDOWN_TIMEOUT).is_ok());
        assert!(BackgroundTaskShutdownPolicy::new(
            MAX_BACKGROUND_TASK_SHUTDOWN_TIMEOUT + Duration::from_millis(1)
        )
        .is_err());
    }

    #[test]
    fn failure_latch_preserves_the_first_reported_root_cause() {
        let latch = BackgroundTaskFailureLatch::new();
        let receiver = latch.subscribe();
        let first = BackgroundTaskFailure::unexpected_exit("first-task".into());
        let later = BackgroundTaskFailure::panicked("later-task".into(), Box::new("later panic"));

        assert!(latch.report(first.clone()));
        assert!(!latch.report(later));
        assert_eq!(receiver.borrow().as_ref(), Some(&first));
    }
}
