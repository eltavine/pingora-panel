use crate::{
    ClaimRequest, Finish, Job, JobError, JobKind, JobStore, Lease, Progress, Renewal, RetryPolicy,
    ScheduleName, ScheduleStore,
};
use async_trait::async_trait;
use chrono::Utc;
use futures_util::FutureExt;
use panel_errors::{PanelError, Result};
use std::{collections::HashMap, panic::AssertUnwindSafe, sync::Arc, time::Duration};
use tokio::sync::{watch, Semaphore};
use tokio_util::{sync::CancellationToken, task::TaskTracker};

/// Runs jobs of one kind.
///
/// Returning `Ok` succeeds the job. An error retries it when retryable and
/// attempts remain, and fails it otherwise. A handler should stop promptly
/// once its cancellation token fires; the job then ends cancelled. A panic
/// counts as a retryable failure.
#[async_trait]
pub trait JobHandler: Send + Sync {
    async fn run(&self, context: JobContext) -> Result<()>;
}

/// The job an attempt runs and its controls.
pub struct JobContext {
    job: Job,
    attempt: u32,
    cancellation: CancellationToken,
    progress: watch::Sender<Option<Progress>>,
}

impl JobContext {
    pub fn job(&self) -> &Job {
        &self.job
    }

    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    /// Fires when cancellation is requested or the lease is lost.
    pub fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    /// Records progress. Reports are coalesced and written at most once per
    /// progress interval; the latest one is always written.
    pub fn report(&self, percent: u8, message: impl Into<String>) -> Result<()> {
        let progress = Progress::new(percent, message)?;
        self.progress.send_replace(Some(progress));
        Ok(())
    }
}

/// Worker tuning.
#[derive(Clone, Debug)]
pub struct WorkerOptions {
    pub owner: String,
    pub concurrency: usize,
    pub lease: Duration,
    pub poll_interval: Duration,
    pub retry: RetryPolicy,
    /// How long running jobs may finish after shutdown before their leases
    /// are left to expire.
    pub shutdown_grace: Duration,
    /// The shortest time between two progress writes of one job.
    pub progress_interval: Duration,
}

impl WorkerOptions {
    pub fn new(owner: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            concurrency: 4,
            lease: Duration::from_secs(30),
            poll_interval: Duration::from_secs(1),
            retry: RetryPolicy::default(),
            shutdown_grace: Duration::from_secs(20),
            progress_interval: Duration::from_secs(1),
        }
    }
}

/// Leases jobs of the kinds it has handlers for and runs them.
pub struct Worker {
    store: Arc<dyn JobStore>,
    windows: Option<Arc<dyn ScheduleStore>>,
    handlers: HashMap<JobKind, Arc<dyn JobHandler>>,
    options: WorkerOptions,
}

impl Worker {
    pub fn new(store: Arc<dyn JobStore>, options: WorkerOptions) -> Self {
        Self {
            store,
            windows: None,
            handlers: HashMap::new(),
            options,
        }
    }

    pub fn with_handler(mut self, kind: JobKind, handler: Arc<dyn JobHandler>) -> Self {
        self.handlers.insert(kind, handler);
        self
    }

    /// Lets jobs that require a maintenance window start while it is open.
    pub fn with_windows(mut self, windows: Arc<dyn ScheduleStore>) -> Self {
        self.windows = Some(windows);
        self
    }

    /// Runs until `shutdown` fires, then lets running jobs finish within
    /// the grace period.
    pub async fn run(self, shutdown: CancellationToken) {
        let kinds: Vec<JobKind> = self.handlers.keys().cloned().collect();
        let permits = Arc::new(Semaphore::new(self.options.concurrency.max(1)));
        let running = TaskTracker::new();
        let stop_jobs = CancellationToken::new();
        let shared = Arc::new(self);
        while !shutdown.is_cancelled() && !kinds.is_empty() {
            let available = permits.available_permits();
            let claimed = if available == 0 {
                0
            } else {
                match shared.claim(&kinds, available).await {
                    Ok(leases) => {
                        let count = leases.len();
                        for lease in leases {
                            let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                                break;
                            };
                            let worker = Arc::clone(&shared);
                            let stop = stop_jobs.child_token();
                            running.spawn(async move {
                                worker.attempt(lease, stop).await;
                                drop(permit);
                            });
                        }
                        count
                    }
                    Err(error) => {
                        tracing::warn!(error_code = %error.code, "job claim failed");
                        0
                    }
                }
            };
            if claimed == 0 {
                tokio::select! {
                    () = shutdown.cancelled() => {}
                    () = tokio::time::sleep(shared.options.poll_interval) => {}
                }
            }
        }
        running.close();
        if tokio::time::timeout(shared.options.shutdown_grace, running.wait())
            .await
            .is_err()
        {
            stop_jobs.cancel();
            tracing::warn!("running jobs did not finish in time; their leases will expire");
        }
    }

    async fn claim(&self, kinds: &[JobKind], limit: usize) -> Result<Vec<Lease>> {
        let open_windows: Vec<ScheduleName> = match &self.windows {
            Some(store) => {
                let now = Utc::now();
                store
                    .windows()
                    .await?
                    .into_iter()
                    .filter(|window| window.is_open(now))
                    .map(|window| window.name().clone())
                    .collect()
            }
            None => Vec::new(),
        };
        self.store
            .claim(&ClaimRequest {
                owner: &self.options.owner,
                kinds,
                limit,
                lease: self.options.lease,
                open_windows: &open_windows,
            })
            .await
    }

    /// Runs one attempt, renewing its lease and recording its outcome.
    async fn attempt(&self, lease: Lease, stop: CancellationToken) {
        let Some(handler) = self.handlers.get(&lease.job.kind).cloned() else {
            return;
        };
        let cancellation = stop.child_token();
        let (progress, reported) = watch::channel(None);
        let context = JobContext {
            job: lease.job.clone(),
            attempt: lease.attempt,
            cancellation: cancellation.clone(),
            progress,
        };
        let execution = AssertUnwindSafe(handler.run(context)).catch_unwind();
        tokio::pin!(execution);
        let mut renewal = tokio::time::interval(self.options.lease / 3);
        renewal.tick().await;
        let mut progress_tick = tokio::time::interval(self.options.progress_interval);
        let mut written: Option<Progress> = None;
        let result = loop {
            tokio::select! {
                result = &mut execution => break result,
                _ = renewal.tick() => match self.store.renew(&lease, self.options.lease).await {
                    Ok(Renewal::Held { cancel_requested }) => {
                        if cancel_requested {
                            cancellation.cancel();
                        }
                    }
                    Ok(Renewal::Lost) => {
                        cancellation.cancel();
                        tracing::warn!(job_id = %lease.job.id, "job lease lost; abandoning the attempt");
                        return;
                    }
                    Err(error) => tracing::warn!(job_id = %lease.job.id, error_code = %error.code, "job lease renewal failed"),
                },
                _ = progress_tick.tick() => {
                    let latest = reported.borrow().clone();
                    if let Some(progress) = latest.filter(|latest| written.as_ref() != Some(latest)) {
                        match self.store.report_progress(&lease, &progress).await {
                            Ok(_) => written = Some(progress),
                            Err(error) => tracing::warn!(job_id = %lease.job.id, error_code = %error.code, "job progress not recorded"),
                        }
                    }
                },
            }
        };
        // The handler's sender is gone by now, so compare with what was
        // written instead of asking the channel what changed.
        let latest = reported.borrow().clone();
        if let Some(progress) = latest.filter(|latest| written.as_ref() != Some(latest)) {
            let _ = self.store.report_progress(&lease, &progress).await;
        }
        let finish = match result {
            Ok(Ok(())) => Finish::Succeeded,
            Ok(Err(_)) if cancellation.is_cancelled() && !stop.is_cancelled() => Finish::Cancelled,
            Ok(Err(error)) => self.after_failure(&lease, JobError::from(&error), error.retryable),
            Err(_) => self.after_failure(
                &lease,
                JobError::from(&PanelError::internal("job handler panicked")),
                true,
            ),
        };
        match self.store.finish(&lease, finish).await {
            Ok(true) => {}
            Ok(false) => {
                tracing::warn!(job_id = %lease.job.id, "job lease lost before its outcome was recorded")
            }
            Err(error) => {
                tracing::warn!(job_id = %lease.job.id, error_code = %error.code, "job outcome not recorded; the lease will expire")
            }
        }
    }

    fn after_failure(&self, lease: &Lease, error: JobError, retryable: bool) -> Finish {
        if retryable && lease.attempt < lease.job.max_attempts {
            let delay = self.options.retry.delay(lease.attempt);
            Finish::Retry {
                at: Utc::now() + crate::time::duration(delay),
                error,
            }
        } else {
            Finish::Failed { error }
        }
    }
}

/// Fires due schedules every `interval` until `shutdown`.
pub async fn run_scheduler(
    store: Arc<dyn ScheduleStore>,
    interval: Duration,
    shutdown: CancellationToken,
) {
    loop {
        match store.fire_due(Utc::now()).await {
            Ok(0) => {}
            Ok(created) => tracing::info!(created, "scheduled jobs enqueued"),
            Err(error) => tracing::warn!(error_code = %error.code, "schedule firing failed"),
        }
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(interval) => {}
        }
    }
}
