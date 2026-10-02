use crate::{
    Job, JobError, JobId, JobKind, JobSpec, JobState, MaintenanceWindow, Progress, Schedule,
    ScheduleName,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use panel_errors::Result;
use std::time::Duration;

/// The result of enqueuing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Enqueued {
    pub job_id: JobId,
    /// False when the kind and idempotency key named an existing job.
    pub created: bool,
}

/// What a worker asks to lease.
#[derive(Clone, Debug)]
pub struct ClaimRequest<'a> {
    /// Identifies the worker; leases are fenced by owner and attempt.
    pub owner: &'a str,
    pub kinds: &'a [JobKind],
    pub limit: usize,
    pub lease: Duration,
    /// Maintenance windows open now; jobs requiring another wait.
    pub open_windows: &'a [ScheduleName],
}

/// One attempt of a job, leased to one worker until `expires_at`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Lease {
    pub job: Job,
    pub owner: String,
    pub attempt: u32,
    pub expires_at: DateTime<Utc>,
}

/// The state of a lease when it is renewed.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Renewal {
    Held {
        cancel_requested: bool,
    },
    /// The lease expired and another worker may hold the job.
    Lost,
}

/// How an attempt ended.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Finish {
    Succeeded,
    Cancelled,
    Retry { at: DateTime<Utc>, error: JobError },
    Failed { error: JobError },
}

/// The result of a cancellation request.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CancelOutcome {
    /// The job had not started and will not run.
    Cancelled,
    /// The job is running; its worker is asked to stop.
    Requested,
    AlreadyFinished(JobState),
}

/// Durable jobs with expiring, fenced leases.
///
/// Implementations claim each job for at most one live lease. A lease that
/// expires lets another worker run the job's next attempt, except that an
/// expired job whose cancellation was requested becomes cancelled and one on
/// its final attempt becomes failed. Every write with a lease is fenced by
/// its owner and attempt, so a worker that lost its lease changes nothing.
#[async_trait]
pub trait JobStore: Send + Sync {
    async fn enqueue(&self, spec: &JobSpec) -> Result<Enqueued>;

    async fn claim(&self, request: &ClaimRequest<'_>) -> Result<Vec<Lease>>;

    async fn renew(&self, lease: &Lease, extend_by: Duration) -> Result<Renewal>;

    /// Returns false when the lease is no longer held.
    async fn report_progress(&self, lease: &Lease, progress: &Progress) -> Result<bool>;

    /// Returns false when the lease is no longer held.
    async fn finish(&self, lease: &Lease, finish: Finish) -> Result<bool>;

    async fn cancel(&self, job_id: JobId) -> Result<CancelOutcome>;

    async fn get(&self, job_id: JobId) -> Result<Option<Job>>;
}

/// Durable schedules and maintenance windows.
#[async_trait]
pub trait ScheduleStore: Send + Sync {
    /// Creates or replaces a schedule; it next fires at its first occurrence
    /// after `now`.
    async fn save_schedule(&self, schedule: &Schedule, now: DateTime<Utc>) -> Result<()>;

    async fn save_window(&self, window: &MaintenanceWindow) -> Result<()>;

    async fn windows(&self) -> Result<Vec<MaintenanceWindow>>;

    /// Enqueues one job for each due occurrence of every enabled schedule
    /// and advances the schedules, exactly once per occurrence even with
    /// several schedulers. Occurrences missed while no scheduler ran
    /// collapse into one. Returns the number of jobs created.
    async fn fire_due(&self, now: DateTime<Utc>) -> Result<usize>;
}
