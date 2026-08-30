use futures_util::FutureExt;
use std::{
    any::Any,
    future::Future,
    panic::AssertUnwindSafe,
    sync::{Arc, Mutex},
};
use tokio::{sync::watch, task::JoinHandle};

#[derive(Clone)]
pub struct BackgroundTaskSupervisor {
    inner: Arc<BackgroundTaskSupervisorInner>,
}

struct BackgroundTaskSupervisorInner {
    shutdown: watch::Sender<bool>,
    failure: watch::Sender<Option<BackgroundTaskFailure>>,
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
        let (failure, _) = watch::channel(None);
        Self {
            inner: Arc::new(BackgroundTaskSupervisorInner {
                shutdown,
                failure,
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
                    failure_sender.send_replace(Some(failure.clone()));
                    Err(failure)
                }
                Err(panic) => {
                    let failure = BackgroundTaskFailure::panicked(task_name, panic);
                    failure_sender.send_replace(Some(failure.clone()));
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
        let tasks = {
            let mut registry = self
                .inner
                .registry
                .lock()
                .expect("background task registry mutex poisoned");
            registry.shutting_down = true;
            let _ = self.inner.shutdown.send(true);
            std::mem::take(&mut registry.tasks)
        };
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
                first_error.get_or_insert_with(|| BackgroundTaskError::from_failure(failure));
            }
        }
        first_error.map_or(Ok(()), Err)
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
}
