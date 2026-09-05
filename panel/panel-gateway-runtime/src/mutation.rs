//! Structured execution and bounded admission for durable gateway mutations.
//!
//! Serialization and task lifetime are intentionally kept outside the engine
//! state mutex. Read-only status calls therefore never wait on storage I/O,
//! while request cancellation cannot cancel an admitted durable mutation.

use crate::mutation_lifecycle::MutationLifecycle;
use panel_errors::{PanelError, Result};
use std::{
    future::Future,
    num::NonZeroUsize,
    sync::{Arc, Mutex as StdMutex, MutexGuard as StdMutexGuard},
};
use tokio::{
    sync::{Mutex, Semaphore, TryAcquireError},
    task::JoinHandle,
};
use tokio_util::task::TaskTracker;

pub const DEFAULT_MAX_PENDING_MUTATIONS: usize = 64;
pub const MAX_PENDING_MUTATIONS: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GatewayMutationCapacity(NonZeroUsize);

impl GatewayMutationCapacity {
    pub fn new(capacity: usize) -> Result<Self> {
        let capacity = NonZeroUsize::new(capacity).ok_or_else(|| {
            PanelError::invalid_argument("gateway mutation capacity must be greater than zero")
        })?;
        if capacity.get() > MAX_PENDING_MUTATIONS {
            return Err(PanelError::invalid_argument(format!(
                "gateway mutation capacity must not exceed {MAX_PENDING_MUTATIONS}"
            )));
        }
        Ok(Self(capacity))
    }

    pub const fn get(self) -> usize {
        self.0.get()
    }
}

impl Default for GatewayMutationCapacity {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_PENDING_MUTATIONS)
            .expect("default gateway mutation capacity is valid")
    }
}

struct GatewayMutationExecutorInner {
    gate: Mutex<()>,
    admission: Arc<Semaphore>,
    lifecycle: StdMutex<MutationLifecycle>,
    tasks: TaskTracker,
    capacity: usize,
}

/// A cloneable, bounded executor for request-independent durable mutations.
///
/// A short synchronous lifecycle lock makes admission atomic with shutdown.
/// The Tokio semaphore bounds running plus queued work, the async mutex
/// serializes transactions, and `TaskTracker` owns their graceful lifetime.
#[derive(Clone)]
pub struct GatewayMutationExecutor {
    inner: Arc<GatewayMutationExecutorInner>,
}

impl GatewayMutationExecutor {
    pub fn new() -> Self {
        Self::with_capacity(GatewayMutationCapacity::default())
    }

    pub fn with_capacity(capacity: GatewayMutationCapacity) -> Self {
        Self::from_capacity(capacity.get())
    }

    fn from_capacity(capacity: usize) -> Self {
        Self {
            inner: Arc::new(GatewayMutationExecutorInner {
                gate: Mutex::new(()),
                admission: Arc::new(Semaphore::new(capacity)),
                lifecycle: StdMutex::new(MutationLifecycle::default()),
                tasks: TaskTracker::new(),
                capacity,
            }),
        }
    }

    pub fn capacity(&self) -> usize {
        self.inner.capacity
    }

    pub fn available_capacity(&self) -> usize {
        self.inner.admission.available_permits()
    }

    pub fn pending_tasks(&self) -> usize {
        self.inner.tasks.len()
    }

    pub fn is_closed(&self) -> bool {
        self.lifecycle().is_closed()
    }

    /// Atomically stop admission before allowing the tracker to drain.
    pub fn close(&self) -> bool {
        let mut lifecycle = self.lifecycle();
        if !lifecycle.close() {
            return false;
        }
        self.inner.admission.close();
        self.inner.tasks.close();
        true
    }

    pub async fn wait(&self) {
        self.inner.tasks.wait().await;
    }

    pub(crate) fn spawn<T>(
        &self,
        operation: impl Future<Output = Result<T>> + Send + 'static,
    ) -> Result<JoinHandle<Result<T>>>
    where
        T: Send + 'static,
    {
        let lifecycle = self.lifecycle();
        if lifecycle.is_closed() {
            return Err(PanelError::precondition_failed(
                "gateway mutation executor is closed",
            ));
        }
        let permit = Arc::clone(&self.inner.admission)
            .try_acquire_owned()
            .map_err(Self::admission_error)?;
        let inner = Arc::clone(&self.inner);
        let task = self.inner.tasks.spawn(async move {
            let _permit = permit;
            let _guard = inner.gate.lock().await;
            operation.await
        });
        drop(lifecycle);
        Ok(task)
    }

    fn lifecycle(&self) -> StdMutexGuard<'_, MutationLifecycle> {
        self.inner
            .lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn admission_error(error: TryAcquireError) -> PanelError {
        match error {
            TryAcquireError::Closed => {
                PanelError::precondition_failed("gateway mutation executor is closed")
            }
            TryAcquireError::NoPermits => {
                PanelError::resource_exhausted("gateway mutation capacity is exhausted")
            }
        }
    }
}

impl Default for GatewayMutationExecutor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel_errors::ErrorCode;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn serializes_operations_and_waits_for_completion() {
        let executor =
            GatewayMutationExecutor::with_capacity(GatewayMutationCapacity::new(2).unwrap());
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(Semaphore::new(0));

        for _ in 0..2 {
            let active = Arc::clone(&active);
            let maximum = Arc::clone(&maximum);
            let release = Arc::clone(&release);
            executor
                .spawn(async move {
                    let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                    maximum.fetch_max(current, Ordering::SeqCst);
                    release.acquire().await.unwrap().forget();
                    active.fetch_sub(1, Ordering::SeqCst);
                    Ok(())
                })
                .unwrap();
        }

        while active.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        assert_eq!(maximum.load(Ordering::SeqCst), 1);
        assert_eq!(executor.pending_tasks(), 2);
        assert_eq!(executor.available_capacity(), 0);

        assert!(executor.close());
        assert!(!executor.close());
        release.add_permits(2);
        executor.wait().await;
        assert_eq!(maximum.load(Ordering::SeqCst), 1);
        assert_eq!(executor.pending_tasks(), 0);
    }

    #[tokio::test]
    async fn rejects_excess_and_post_shutdown_mutations() {
        let executor =
            GatewayMutationExecutor::with_capacity(GatewayMutationCapacity::new(1).unwrap());
        let release = Arc::new(Semaphore::new(0));
        let operation_release = Arc::clone(&release);
        executor
            .spawn(async move {
                operation_release.acquire().await.unwrap().forget();
                Ok(())
            })
            .unwrap();

        let saturated = executor.spawn(async { Ok(()) }).unwrap_err();
        assert_eq!(saturated.code.as_str(), ErrorCode::RESOURCE_EXHAUSTED);

        executor.close();
        let closed = executor.spawn(async { Ok(()) }).unwrap_err();
        assert_eq!(closed.code.as_str(), ErrorCode::PRECONDITION_FAILED);
        release.add_permits(1);
        executor.wait().await;
    }

    #[test]
    fn capacity_is_validated_without_reaching_tokio_panics() {
        assert_eq!(
            GatewayMutationCapacity::new(0).err().unwrap().code.as_str(),
            ErrorCode::INVALID_ARGUMENT
        );
        assert_eq!(
            GatewayMutationCapacity::new(MAX_PENDING_MUTATIONS + 1)
                .err()
                .unwrap()
                .code
                .as_str(),
            ErrorCode::INVALID_ARGUMENT
        );
        assert_eq!(
            GatewayMutationExecutor::new().capacity(),
            DEFAULT_MAX_PENDING_MUTATIONS
        );
    }
}
