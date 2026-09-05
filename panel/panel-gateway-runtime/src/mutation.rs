//! Structured execution for durable gateway mutations.
//!
//! Serialization and task lifetime are intentionally kept outside the engine
//! state mutex. Read-only status calls therefore never wait on storage I/O,
//! while request cancellation cannot cancel an in-flight durable mutation.

use panel_errors::Result;
use std::{future::Future, sync::Arc};
use tokio::{sync::Mutex, task::JoinHandle};
use tokio_util::task::TaskTracker;

#[derive(Clone, Default)]
pub struct GatewayMutationExecutor {
    gate: Arc<Mutex<()>>,
    tasks: TaskTracker,
}

impl GatewayMutationExecutor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn pending_tasks(&self) -> usize {
        self.tasks.len()
    }

    pub fn close(&self) -> bool {
        self.tasks.close()
    }

    pub async fn wait(&self) {
        self.tasks.wait().await;
    }

    pub(crate) fn spawn<T>(
        &self,
        operation: impl Future<Output = Result<T>> + Send + 'static,
    ) -> JoinHandle<Result<T>>
    where
        T: Send + 'static,
    {
        let gate = Arc::clone(&self.gate);
        self.tasks.spawn(async move {
            let _guard = gate.lock().await;
            operation.await
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::Semaphore;

    #[tokio::test]
    async fn serializes_operations_and_waits_for_completion() {
        let executor = GatewayMutationExecutor::new();
        let active = Arc::new(AtomicUsize::new(0));
        let maximum = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(Semaphore::new(0));

        for _ in 0..2 {
            let active = Arc::clone(&active);
            let maximum = Arc::clone(&maximum);
            let release = Arc::clone(&release);
            executor.spawn(async move {
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                maximum.fetch_max(current, Ordering::SeqCst);
                release.acquire().await.unwrap().forget();
                active.fetch_sub(1, Ordering::SeqCst);
                Ok(())
            });
        }

        while active.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
        assert_eq!(maximum.load(Ordering::SeqCst), 1);
        assert_eq!(executor.pending_tasks(), 2);

        executor.close();
        release.add_permits(2);
        executor.wait().await;
        assert_eq!(maximum.load(Ordering::SeqCst), 1);
        assert_eq!(executor.pending_tasks(), 0);
    }
}
