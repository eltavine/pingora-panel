#![forbid(unsafe_code)]

//! Durable jobs, independent of storage.
//!
//! A job is leased to one worker at a time under an expiring lease that the
//! worker renews; a crashed worker's job runs again once its lease expires.
//! Cancellation is cooperative, failed attempts are retried with
//! exponential backoff, and progress is recorded while a job runs.
//! Schedules enqueue a job at every occurrence of an RFC 5545 recurrence,
//! and maintenance windows hold back the jobs that require them until an
//! occurrence of the window is open.

mod memory;
mod model;
mod policy;
mod recurrence;
mod store;
mod worker;

pub use memory::MemoryJobStore;
pub use model::{
    Job, JobError, JobId, JobKind, JobOrigin, JobSpec, JobState, Progress, ScheduleName,
};
pub use policy::RetryPolicy;
pub use recurrence::{JobTemplate, MaintenanceWindow, Recurrence, Schedule};
pub use store::{
    CancelOutcome, ClaimRequest, Enqueued, Finish, JobStore, Lease, Renewal, ScheduleStore,
};
pub use worker::{run_scheduler, JobContext, JobHandler, Worker, WorkerOptions};
