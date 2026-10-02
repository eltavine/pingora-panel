use crate::{
    CancelOutcome, ClaimRequest, Enqueued, Finish, Job, JobError, JobId, JobKind, JobSpec,
    JobState, JobStore, Lease, MaintenanceWindow, Progress, Renewal, Schedule, ScheduleName,
    ScheduleStore,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use panel_context::IdempotencyKey;
use panel_errors::{PanelError, Result};
use std::{collections::BTreeMap, time::Duration};
use tokio::sync::Mutex;

struct Stored {
    job: Job,
    lease: Option<(String, DateTime<Utc>)>,
}

#[derive(Default)]
struct State {
    jobs: BTreeMap<JobId, Stored>,
    keys: BTreeMap<(JobKind, IdempotencyKey), JobId>,
    schedules: BTreeMap<ScheduleName, (Schedule, Option<DateTime<Utc>>)>,
    windows: BTreeMap<ScheduleName, MaintenanceWindow>,
}

/// Jobs in memory, with the semantics required of durable stores; for tests
/// and single-process tools.
#[derive(Default)]
pub struct MemoryJobStore {
    state: Mutex<State>,
}

impl MemoryJobStore {
    pub fn new() -> Self {
        Self::default()
    }
}

fn duration(value: Duration) -> chrono::Duration {
    chrono::Duration::from_std(value).unwrap_or(chrono::Duration::MAX)
}

fn holds(stored: &Stored, lease: &Lease, now: DateTime<Utc>) -> bool {
    stored.job.state == JobState::Running
        && stored.job.attempts == lease.attempt
        && stored
            .lease
            .as_ref()
            .is_some_and(|(owner, expires)| *owner == lease.owner && *expires >= now)
}

fn insert(state: &mut State, spec: &JobSpec, now: DateTime<Utc>) -> Enqueued {
    let key = (spec.kind.clone(), spec.idempotency_key.clone());
    if let Some(job_id) = state.keys.get(&key) {
        return Enqueued {
            job_id: *job_id,
            created: false,
        };
    }
    let job_id = JobId::generate();
    state.keys.insert(key, job_id);
    state.jobs.insert(
        job_id,
        Stored {
            job: Job {
                id: job_id,
                kind: spec.kind.clone(),
                idempotency_key: spec.idempotency_key.clone(),
                state: JobState::Queued,
                attempts: 0,
                max_attempts: spec.max_attempts,
                media_type: spec.media_type.clone(),
                payload: spec.payload.clone(),
                priority: spec.priority,
                run_after: spec.not_before.unwrap_or(now).max(now),
                maintenance_window: spec.maintenance_window.clone(),
                cancel_requested: false,
                progress: None,
                last_error: None,
                origin: spec.origin.clone(),
                created_at: now,
                updated_at: now,
                finished_at: None,
            },
            lease: None,
        },
    );
    Enqueued {
        job_id,
        created: true,
    }
}

#[async_trait]
impl JobStore for MemoryJobStore {
    async fn enqueue(&self, spec: &JobSpec) -> Result<Enqueued> {
        spec.validate()?;
        Ok(insert(&mut *self.state.lock().await, spec, Utc::now()))
    }

    async fn claim(&self, request: &ClaimRequest<'_>) -> Result<Vec<Lease>> {
        let now = Utc::now();
        let mut state = self.state.lock().await;
        for stored in state.jobs.values_mut() {
            let expired = stored.job.state == JobState::Running
                && stored
                    .lease
                    .as_ref()
                    .is_some_and(|(_, expires)| *expires < now);
            if expired && stored.job.cancel_requested {
                stored.job.state = JobState::Cancelled;
                stored.job.finished_at = Some(now);
                stored.lease = None;
            } else if expired && stored.job.attempts >= stored.job.max_attempts {
                stored.job.state = JobState::Failed;
                stored.job.last_error = Some(JobError {
                    code: panel_errors::ErrorCode::DEADLINE_EXCEEDED.into(),
                    message: "the lease expired on the final attempt".into(),
                });
                stored.job.finished_at = Some(now);
                stored.lease = None;
            }
        }
        let mut eligible: Vec<&mut Stored> = state
            .jobs
            .values_mut()
            .filter(|stored| {
                let job = &stored.job;
                let ready = match job.state {
                    JobState::Queued | JobState::Retrying => {
                        job.run_after <= now && !job.cancel_requested
                    }
                    JobState::Running => stored
                        .lease
                        .as_ref()
                        .is_some_and(|(_, expires)| *expires < now),
                    _ => false,
                };
                ready
                    && request.kinds.contains(&job.kind)
                    && job
                        .maintenance_window
                        .as_ref()
                        .is_none_or(|window| request.open_windows.contains(window))
            })
            .collect();
        eligible.sort_by(|a, b| {
            (b.job.priority, a.job.run_after, a.job.id).cmp(&(
                a.job.priority,
                b.job.run_after,
                b.job.id,
            ))
        });
        let expires_at = now + duration(request.lease);
        Ok(eligible
            .into_iter()
            .take(request.limit)
            .map(|stored| {
                stored.job.state = JobState::Running;
                stored.job.attempts += 1;
                stored.job.updated_at = now;
                stored.lease = Some((request.owner.to_owned(), expires_at));
                Lease {
                    job: stored.job.clone(),
                    owner: request.owner.to_owned(),
                    attempt: stored.job.attempts,
                    expires_at,
                }
            })
            .collect())
    }

    async fn renew(&self, lease: &Lease, extend_by: Duration) -> Result<Renewal> {
        let now = Utc::now();
        let mut state = self.state.lock().await;
        let Some(stored) = state.jobs.get_mut(&lease.job.id) else {
            return Ok(Renewal::Lost);
        };
        if !holds(stored, lease, now) {
            return Ok(Renewal::Lost);
        }
        stored.lease = Some((lease.owner.clone(), now + duration(extend_by)));
        Ok(Renewal::Held {
            cancel_requested: stored.job.cancel_requested,
        })
    }

    async fn report_progress(&self, lease: &Lease, progress: &Progress) -> Result<bool> {
        let now = Utc::now();
        let mut state = self.state.lock().await;
        match state.jobs.get_mut(&lease.job.id) {
            Some(stored) if holds(stored, lease, now) => {
                stored.job.progress = Some(progress.clone());
                stored.job.updated_at = now;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn finish(&self, lease: &Lease, finish: Finish) -> Result<bool> {
        let now = Utc::now();
        let mut state = self.state.lock().await;
        let Some(stored) = state.jobs.get_mut(&lease.job.id) else {
            return Ok(false);
        };
        if !holds(stored, lease, now) {
            return Ok(false);
        }
        stored.lease = None;
        stored.job.updated_at = now;
        match finish {
            Finish::Succeeded => stored.job.state = JobState::Succeeded,
            Finish::Cancelled => stored.job.state = JobState::Cancelled,
            Finish::Failed { error } => {
                stored.job.state = JobState::Failed;
                stored.job.last_error = Some(error);
            }
            Finish::Retry { at, error } => {
                stored.job.state = JobState::Retrying;
                stored.job.run_after = at;
                stored.job.last_error = Some(error);
            }
        }
        if stored.job.state.is_final() {
            stored.job.finished_at = Some(now);
        }
        Ok(true)
    }

    async fn cancel(&self, job_id: JobId) -> Result<CancelOutcome> {
        let now = Utc::now();
        let mut state = self.state.lock().await;
        let stored = state
            .jobs
            .get_mut(&job_id)
            .ok_or_else(|| PanelError::not_found(format!("job {job_id} does not exist")))?;
        Ok(match stored.job.state {
            JobState::Queued | JobState::Retrying => {
                stored.job.state = JobState::Cancelled;
                stored.job.cancel_requested = true;
                stored.job.finished_at = Some(now);
                CancelOutcome::Cancelled
            }
            JobState::Running => {
                stored.job.cancel_requested = true;
                CancelOutcome::Requested
            }
            state => CancelOutcome::AlreadyFinished(state),
        })
    }

    async fn get(&self, job_id: JobId) -> Result<Option<Job>> {
        Ok(self
            .state
            .lock()
            .await
            .jobs
            .get(&job_id)
            .map(|stored| stored.job.clone()))
    }
}

#[async_trait]
impl ScheduleStore for MemoryJobStore {
    async fn save_schedule(&self, schedule: &Schedule, now: DateTime<Utc>) -> Result<()> {
        let next = schedule.recurrence.next_after(now);
        self.state
            .lock()
            .await
            .schedules
            .insert(schedule.name.clone(), (schedule.clone(), next));
        Ok(())
    }

    async fn save_window(&self, window: &MaintenanceWindow) -> Result<()> {
        self.state
            .lock()
            .await
            .windows
            .insert(window.name().clone(), window.clone());
        Ok(())
    }

    async fn windows(&self) -> Result<Vec<MaintenanceWindow>> {
        Ok(self.state.lock().await.windows.values().cloned().collect())
    }

    async fn fire_due(&self, now: DateTime<Utc>) -> Result<usize> {
        let mut state = self.state.lock().await;
        let due: Vec<(Schedule, DateTime<Utc>)> = state
            .schedules
            .values()
            .filter_map(|(schedule, next)| {
                next.filter(|next| schedule.enabled && *next <= now)
                    .map(|next| (schedule.clone(), next))
            })
            .collect();
        let mut created = 0;
        for (schedule, occurrence) in due {
            let spec = schedule.occurrence(occurrence)?;
            if insert(&mut state, &spec, now).created {
                created += 1;
            }
            let next = schedule.recurrence.next_after(now);
            state
                .schedules
                .insert(schedule.name.clone(), (schedule, next));
        }
        Ok(created)
    }
}
