use async_trait::async_trait;
use chrono::{DateTime, TimeDelta, Utc};
use panel_errors::{PanelError, Result};
use panel_event_contracts::automation::v1 as event;
use panel_events::{
    Actor, AggregateId, AggregateRef, AggregateType, EventDraft, EventEnvelope, EventOrigin,
    IdempotencyKey, Principal, RequestId, RequestScope, ServiceName,
};
use panel_jobs::{
    CancelOutcome, ClaimRequest, Enqueued, Finish, Job, JobError, JobId, JobKind, JobOrigin,
    JobSpec, JobState, JobStore, JobTemplate, Lease, MaintenanceWindow, Progress, Recurrence,
    Renewal, Schedule, ScheduleName, ScheduleStore,
};
use panel_sqlite::{storage_error, ServiceDatabase, SqliteOutbox};
use sqlx::{sqlite::SqliteRow, types::Json, Row, SqliteConnection};
use std::{str::FromStr, time::Duration};

/// The columns read into a [`Job`], as a literal so queries stay static.
macro_rules! columns {
    () => {
        "job_id, kind, idempotency_key, state, attempts, max_attempts, media_type, payload, \
         priority, run_after, maintenance_window, cancel_requested, lease_owner, \
         lease_expires_at, progress_percent, progress_message, last_error_code, \
         last_error_message, correlation_id, causation_id, created_at, updated_at, finished_at"
    };
}

/// Matches a job only while `?1` is leased to owner `?2` for attempt `?3`
/// at `?4`, the time now.
macro_rules! lease_fence {
    () => {
        "job_id = ?1 AND state = 'running' AND lease_owner = ?2 AND attempts = ?3 \
         AND lease_expires_at >= ?4"
    };
}

/// Jobs, schedules and maintenance windows in the module's database.
///
/// Every state change and progress report appends a CloudEvent
/// (`automation.job.<change>.v1`) to the outbox in the same transaction, so
/// subscribers see each change exactly when it commits.
#[derive(Clone)]
pub struct SqliteJobStore {
    database: ServiceDatabase,
    producer: ServiceName,
}

/// The time `by` after `now`, or the latest time there is.
fn after(now: DateTime<Utc>, by: Duration) -> DateTime<Utc> {
    TimeDelta::from_std(by)
        .ok()
        .and_then(|by| now.checked_add_signed(by))
        .unwrap_or(DateTime::<Utc>::MAX_UTC)
}

/// What changed about a job; each change is an event type.
#[derive(Clone, Copy)]
enum Change {
    Queued,
    Started,
    Progressed,
    Succeeded,
    Retrying,
    Failed,
    Cancelled,
}

/// A job as its events carry it.
fn job_data(job: &Job) -> event::Job {
    event::Job {
        job_id: job.id.to_string(),
        kind: job.kind.as_str().to_owned(),
        state: job.state.as_str().to_owned(),
        attempt: job.attempts,
        max_attempts: job.max_attempts,
        progress: job.progress.as_ref().map(|progress| event::Progress {
            percent: u32::from(progress.percent()),
            message: progress.message().to_owned(),
        }),
        error: job.last_error.as_ref().map(|error| event::JobError {
            code: error.code.clone(),
            message: error.message.clone(),
        }),
    }
}

fn count(value: u32) -> Result<i32> {
    i32::try_from(value).map_err(|_| PanelError::invalid_argument("count is too large"))
}

fn job(row: &SqliteRow) -> Result<Job> {
    let get = |error: sqlx::Error| storage_error(error);
    let percent: Option<i16> = row.try_get("progress_percent").map_err(get)?;
    let message: Option<String> = row.try_get("progress_message").map_err(get)?;
    let error_code: Option<String> = row.try_get("last_error_code").map_err(get)?;
    let error_message: Option<String> = row.try_get("last_error_message").map_err(get)?;
    let window: Option<String> = row.try_get("maintenance_window").map_err(get)?;
    let unsigned = |value: i32| {
        u32::try_from(value).map_err(|_| PanelError::corrupt_state("stored count is negative"))
    };
    Ok(Job {
        id: JobId::from_uuid(row.try_get("job_id").map_err(get)?),
        kind: JobKind::new(row.try_get::<String, _>("kind").map_err(get)?)?,
        idempotency_key: IdempotencyKey::new(
            row.try_get::<String, _>("idempotency_key").map_err(get)?,
        )?,
        state: JobState::from_str(&row.try_get::<String, _>("state").map_err(get)?)?,
        attempts: unsigned(row.try_get("attempts").map_err(get)?)?,
        max_attempts: unsigned(row.try_get("max_attempts").map_err(get)?)?,
        media_type: row.try_get("media_type").map_err(get)?,
        payload: row.try_get("payload").map_err(get)?,
        priority: row.try_get("priority").map_err(get)?,
        run_after: row.try_get("run_after").map_err(get)?,
        maintenance_window: window.map(ScheduleName::new).transpose()?,
        cancel_requested: row.try_get("cancel_requested").map_err(get)?,
        progress: match (percent, message) {
            (Some(percent), message) => Some(Progress::new(
                u8::try_from(percent)
                    .map_err(|_| PanelError::corrupt_state("stored progress is out of range"))?,
                message.unwrap_or_default(),
            )?),
            (None, _) => None,
        },
        last_error: error_code.map(|code| JobError {
            code,
            message: error_message.unwrap_or_default(),
        }),
        origin: JobOrigin {
            correlation_id: RequestId::new(
                row.try_get::<String, _>("correlation_id").map_err(get)?,
            )?,
            causation_id: RequestId::new(row.try_get::<String, _>("causation_id").map_err(get)?)?,
        },
        created_at: row.try_get("created_at").map_err(get)?,
        updated_at: row.try_get("updated_at").map_err(get)?,
        finished_at: row.try_get("finished_at").map_err(get)?,
    })
}

impl SqliteJobStore {
    pub fn new(database: &ServiceDatabase, producer: ServiceName) -> Self {
        Self {
            database: database.clone(),
            producer,
        }
    }

    fn event(&self, job: &Job, change: Change) -> Result<EventEnvelope> {
        let aggregate = AggregateRef::new(
            AggregateType::new("job")?,
            AggregateId::new(job.id.to_string())?,
        );
        let job_data = Some(job_data(job));
        let draft = match change {
            Change::Queued => EventDraft::of(aggregate, &event::JobQueued { job: job_data }),
            Change::Started => EventDraft::of(aggregate, &event::JobStarted { job: job_data }),
            Change::Progressed => {
                EventDraft::of(aggregate, &event::JobProgressed { job: job_data })
            }
            Change::Succeeded => EventDraft::of(aggregate, &event::JobSucceeded { job: job_data }),
            Change::Retrying => EventDraft::of(aggregate, &event::JobRetrying { job: job_data }),
            Change::Failed => EventDraft::of(aggregate, &event::JobFailed { job: job_data }),
            Change::Cancelled => EventDraft::of(aggregate, &event::JobCancelled { job: job_data }),
        }?;
        let scope = RequestScope::new(job.origin.causation_id.clone())
            .with_correlation_id(job.origin.correlation_id.clone());
        Ok(EventEnvelope::new(
            draft,
            EventOrigin::scoped(
                self.producer.clone(),
                &scope,
                Principal::system(Actor::new(self.producer.as_str())?),
            ),
            Utc::now(),
        ))
    }

    async fn publish(
        &self,
        connection: &mut SqliteConnection,
        job: &Job,
        change: Change,
    ) -> Result<()> {
        SqliteOutbox::append(connection, &self.event(job, change)?).await
    }

    async fn insert(&self, connection: &mut SqliteConnection, spec: &JobSpec) -> Result<Enqueued> {
        spec.validate()?;
        let now = Utc::now();
        let inserted = sqlx::query(concat!(
            "INSERT INTO jobs (job_id, kind, idempotency_key, state, max_attempts, media_type, \
             payload, priority, run_after, maintenance_window, correlation_id, causation_id, \
             created_at, updated_at) \
             VALUES (?1, ?2, ?3, 'queued', ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12) \
             ON CONFLICT (kind, idempotency_key) DO NOTHING RETURNING ",
            columns!()
        ))
        .bind(JobId::generate().as_uuid())
        .bind(spec.kind.as_str())
        .bind(spec.idempotency_key.as_str())
        .bind(count(spec.max_attempts)?)
        .bind(&spec.media_type)
        .bind(&spec.payload)
        .bind(spec.priority)
        .bind(spec.not_before.map_or(now, |at| at.max(now)))
        .bind(spec.maintenance_window.as_ref().map(ScheduleName::as_str))
        .bind(spec.origin.correlation_id.as_str())
        .bind(spec.origin.causation_id.as_str())
        .bind(now)
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?;
        if let Some(row) = inserted {
            let job = job(&row)?;
            self.publish(connection, &job, Change::Queued).await?;
            return Ok(Enqueued {
                job_id: job.id,
                created: true,
            });
        }
        let existing: uuid::Uuid =
            sqlx::query_scalar("SELECT job_id FROM jobs WHERE kind = ?1 AND idempotency_key = ?2")
                .bind(spec.kind.as_str())
                .bind(spec.idempotency_key.as_str())
                .fetch_one(&mut *connection)
                .await
                .map_err(storage_error)?;
        Ok(Enqueued {
            job_id: JobId::from_uuid(existing),
            created: false,
        })
    }
}

#[async_trait]
impl JobStore for SqliteJobStore {
    async fn enqueue(&self, spec: &JobSpec) -> Result<Enqueued> {
        let mut transaction = self.database.begin().await?;
        let enqueued = self.insert(&mut transaction, spec).await?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(enqueued)
    }

    async fn claim(&self, request: &ClaimRequest<'_>) -> Result<Vec<Lease>> {
        let kinds: Vec<&str> = request.kinds.iter().map(JobKind::as_str).collect();
        let windows: Vec<&str> = request
            .open_windows
            .iter()
            .map(ScheduleName::as_str)
            .collect();
        let now = Utc::now();
        let mut transaction = self.database.begin().await?;
        let settled = sqlx::query(concat!(
            "UPDATE jobs SET \
                 state = CASE WHEN cancel_requested THEN 'cancelled' ELSE 'failed' END, \
                 last_error_code = CASE WHEN cancel_requested THEN last_error_code \
                                        ELSE 'DEADLINE_EXCEEDED' END, \
                 last_error_message = CASE WHEN cancel_requested THEN last_error_message \
                                           ELSE 'the lease expired on the final attempt' END, \
                 lease_owner = NULL, lease_expires_at = NULL, \
                 finished_at = ?2, updated_at = ?2 \
             WHERE state = 'running' AND lease_expires_at < ?2 \
               AND kind IN (SELECT value FROM json_each(?1)) \
               AND (cancel_requested OR attempts >= max_attempts) \
             RETURNING ",
            columns!()
        ))
        .bind(Json(&kinds))
        .bind(now)
        .fetch_all(&mut *transaction)
        .await
        .map_err(storage_error)?;
        for row in &settled {
            let job = job(row)?;
            let change = if job.state == JobState::Cancelled {
                Change::Cancelled
            } else {
                Change::Failed
            };
            self.publish(&mut transaction, &job, change).await?;
        }
        let claimed = sqlx::query(concat!(
            "UPDATE jobs SET state = 'running', attempts = attempts + 1, lease_owner = ?1, \
                 lease_expires_at = ?2, updated_at = ?3 \
             WHERE job_id IN ( \
                 SELECT job_id FROM jobs \
                 WHERE kind IN (SELECT value FROM json_each(?4)) \
                   AND (maintenance_window IS NULL \
                        OR maintenance_window IN (SELECT value FROM json_each(?5))) \
                   AND ((state IN ('queued', 'retrying') AND run_after <= ?3 \
                         AND NOT cancel_requested) \
                        OR (state = 'running' AND lease_expires_at < ?3)) \
                 ORDER BY priority DESC, run_after, job_id \
                 LIMIT ?6) \
             RETURNING ",
            columns!()
        ))
        .bind(request.owner)
        .bind(after(now, request.lease))
        .bind(now)
        .bind(Json(&kinds))
        .bind(Json(&windows))
        .bind(i64::try_from(request.limit).unwrap_or(i64::MAX))
        .fetch_all(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let mut leases = Vec::with_capacity(claimed.len());
        for row in &claimed {
            let job = job(row)?;
            self.publish(&mut transaction, &job, Change::Started)
                .await?;
            let expires_at: DateTime<Utc> =
                row.try_get("lease_expires_at").map_err(storage_error)?;
            leases.push(Lease {
                owner: request.owner.to_owned(),
                attempt: job.attempts,
                expires_at,
                job,
            });
        }
        transaction.commit().await.map_err(storage_error)?;
        Ok(leases)
    }

    async fn renew(&self, lease: &Lease, extend_by: Duration) -> Result<Renewal> {
        let now = Utc::now();
        let renewed: Option<bool> = sqlx::query_scalar(concat!(
            "UPDATE jobs SET lease_expires_at = ?5, updated_at = ?4 WHERE ",
            lease_fence!(),
            " RETURNING cancel_requested"
        ))
        .bind(lease.job.id.as_uuid())
        .bind(&lease.owner)
        .bind(count(lease.attempt)?)
        .bind(now)
        .bind(after(now, extend_by))
        .fetch_optional(self.database.pool())
        .await
        .map_err(storage_error)?;
        Ok(match renewed {
            Some(cancel_requested) => Renewal::Held { cancel_requested },
            None => Renewal::Lost,
        })
    }

    async fn report_progress(&self, lease: &Lease, progress: &Progress) -> Result<bool> {
        let mut transaction = self.database.begin().await?;
        let updated = sqlx::query(concat!(
            "UPDATE jobs SET progress_percent = ?5, progress_message = ?6, updated_at = ?4 \
             WHERE ",
            lease_fence!(),
            " RETURNING ",
            columns!()
        ))
        .bind(lease.job.id.as_uuid())
        .bind(&lease.owner)
        .bind(count(lease.attempt)?)
        .bind(Utc::now())
        .bind(i16::from(progress.percent()))
        .bind(progress.message())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let Some(row) = updated else {
            return Ok(false);
        };
        self.publish(&mut transaction, &job(&row)?, Change::Progressed)
            .await?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(true)
    }

    async fn finish(&self, lease: &Lease, finish: Finish) -> Result<bool> {
        let (state, change, run_after, error) = match &finish {
            Finish::Succeeded => ("succeeded", Change::Succeeded, None, None),
            Finish::Cancelled => ("cancelled", Change::Cancelled, None, None),
            Finish::Retry { at, error } => ("retrying", Change::Retrying, Some(*at), Some(error)),
            Finish::Failed { error } => ("failed", Change::Failed, None, Some(error)),
            _ => return Err(PanelError::unsupported_capability("unknown job outcome")),
        };
        let mut transaction = self.database.begin().await?;
        let updated = sqlx::query(concat!(
            "UPDATE jobs SET state = ?5, lease_owner = NULL, lease_expires_at = NULL, \
                 run_after = coalesce(?6, run_after), \
                 last_error_code = coalesce(?7, last_error_code), \
                 last_error_message = coalesce(?8, last_error_message), \
                 finished_at = CASE WHEN ?5 IN ('succeeded', 'failed', 'cancelled') \
                                    THEN ?4 END, \
                 updated_at = ?4 \
             WHERE ",
            lease_fence!(),
            " RETURNING ",
            columns!()
        ))
        .bind(lease.job.id.as_uuid())
        .bind(&lease.owner)
        .bind(count(lease.attempt)?)
        .bind(Utc::now())
        .bind(state)
        .bind(run_after)
        .bind(error.map(|error| error.code.as_str()))
        .bind(error.map(|error| error.message.as_str()))
        .fetch_optional(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let Some(row) = updated else {
            return Ok(false);
        };
        self.publish(&mut transaction, &job(&row)?, change).await?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(true)
    }

    async fn cancel(&self, job_id: JobId) -> Result<CancelOutcome> {
        let mut transaction = self.database.begin().await?;
        let state: Option<String> = sqlx::query_scalar("SELECT state FROM jobs WHERE job_id = ?1")
            .bind(job_id.as_uuid())
            .fetch_optional(&mut *transaction)
            .await
            .map_err(storage_error)?;
        let state = JobState::from_str(
            &state.ok_or_else(|| PanelError::not_found(format!("job {job_id} does not exist")))?,
        )?;
        let outcome = match state {
            JobState::Queued | JobState::Retrying => {
                let row = sqlx::query(concat!(
                    "UPDATE jobs SET state = 'cancelled', cancel_requested = 1, \
                         finished_at = ?2, updated_at = ?2 \
                     WHERE job_id = ?1 RETURNING ",
                    columns!()
                ))
                .bind(job_id.as_uuid())
                .bind(Utc::now())
                .fetch_one(&mut *transaction)
                .await
                .map_err(storage_error)?;
                self.publish(&mut transaction, &job(&row)?, Change::Cancelled)
                    .await?;
                CancelOutcome::Cancelled
            }
            JobState::Running => {
                sqlx::query(
                    "UPDATE jobs SET cancel_requested = 1, updated_at = ?2 WHERE job_id = ?1",
                )
                .bind(job_id.as_uuid())
                .bind(Utc::now())
                .execute(&mut *transaction)
                .await
                .map_err(storage_error)?;
                CancelOutcome::Requested
            }
            state => CancelOutcome::AlreadyFinished(state),
        };
        transaction.commit().await.map_err(storage_error)?;
        Ok(outcome)
    }

    async fn get(&self, job_id: JobId) -> Result<Option<Job>> {
        sqlx::query(concat!(
            "SELECT ",
            columns!(),
            " FROM jobs WHERE job_id = ?1"
        ))
        .bind(job_id.as_uuid())
        .fetch_optional(self.database.pool())
        .await
        .map_err(storage_error)?
        .as_ref()
        .map(job)
        .transpose()
    }
}

fn schedule(row: &SqliteRow) -> Result<Schedule> {
    let get = |error: sqlx::Error| storage_error(error);
    let window: Option<String> = row.try_get("maintenance_window").map_err(get)?;
    let attempts: i32 = row.try_get("max_attempts").map_err(get)?;
    Ok(Schedule {
        name: ScheduleName::new(row.try_get::<String, _>("name").map_err(get)?)?,
        recurrence: Recurrence::parse(row.try_get::<String, _>("recurrence").map_err(get)?)?,
        template: JobTemplate {
            kind: JobKind::new(row.try_get::<String, _>("kind").map_err(get)?)?,
            media_type: row.try_get("media_type").map_err(get)?,
            payload: row.try_get("payload").map_err(get)?,
            max_attempts: u32::try_from(attempts)
                .map_err(|_| PanelError::corrupt_state("stored attempts are negative"))?,
            priority: row.try_get("priority").map_err(get)?,
            maintenance_window: window.map(ScheduleName::new).transpose()?,
        },
        enabled: row.try_get("enabled").map_err(get)?,
    })
}

#[async_trait]
impl ScheduleStore for SqliteJobStore {
    async fn save_schedule(&self, schedule: &Schedule, now: DateTime<Utc>) -> Result<()> {
        sqlx::query(
            "INSERT INTO schedules (name, recurrence, kind, media_type, payload, max_attempts, \
                 priority, maintenance_window, enabled, next_run_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) \
             ON CONFLICT (name) DO UPDATE SET recurrence = excluded.recurrence, \
                 kind = excluded.kind, media_type = excluded.media_type, \
                 payload = excluded.payload, max_attempts = excluded.max_attempts, \
                 priority = excluded.priority, maintenance_window = excluded.maintenance_window, \
                 enabled = excluded.enabled, next_run_at = excluded.next_run_at, \
                 updated_at = excluded.updated_at",
        )
        .bind(schedule.name.as_str())
        .bind(schedule.recurrence.as_str())
        .bind(schedule.template.kind.as_str())
        .bind(&schedule.template.media_type)
        .bind(&schedule.template.payload)
        .bind(count(schedule.template.max_attempts)?)
        .bind(schedule.template.priority)
        .bind(
            schedule
                .template
                .maintenance_window
                .as_ref()
                .map(ScheduleName::as_str),
        )
        .bind(schedule.enabled)
        .bind(schedule.recurrence.next_after(now))
        .bind(now)
        .execute(self.database.pool())
        .await
        .map_err(storage_error)?;
        Ok(())
    }

    async fn save_window(&self, window: &MaintenanceWindow) -> Result<()> {
        sqlx::query(
            "INSERT INTO maintenance_windows (name, recurrence, duration_seconds, updated_at) \
             VALUES (?1, ?2, ?3, ?4) ON CONFLICT (name) DO UPDATE \
             SET recurrence = excluded.recurrence, duration_seconds = excluded.duration_seconds, \
                 updated_at = excluded.updated_at",
        )
        .bind(window.name().as_str())
        .bind(window.recurrence().as_str())
        .bind(i32::try_from(window.duration().as_secs()).unwrap_or(i32::MAX))
        .bind(Utc::now())
        .execute(self.database.pool())
        .await
        .map_err(storage_error)?;
        Ok(())
    }

    async fn windows(&self) -> Result<Vec<MaintenanceWindow>> {
        let rows: Vec<(String, String, i32)> = sqlx::query_as(
            "SELECT name, recurrence, duration_seconds FROM maintenance_windows ORDER BY name",
        )
        .fetch_all(self.database.pool())
        .await
        .map_err(storage_error)?;
        rows.into_iter()
            .map(|(name, recurrence, duration)| {
                MaintenanceWindow::new(
                    ScheduleName::new(name)?,
                    Recurrence::parse(recurrence)?,
                    Duration::from_secs(u64::try_from(duration).unwrap_or_default()),
                )
            })
            .collect()
    }

    async fn fire_due(&self, now: DateTime<Utc>) -> Result<usize> {
        let mut transaction = self.database.begin().await?;
        let due = sqlx::query(
            "SELECT name, recurrence, kind, media_type, payload, max_attempts, priority, \
                    maintenance_window, enabled, next_run_at \
             FROM schedules WHERE enabled AND next_run_at <= ?1 \
             ORDER BY next_run_at",
        )
        .bind(now)
        .fetch_all(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let mut created = 0;
        for row in &due {
            let schedule = schedule(row)?;
            let occurrence: DateTime<Utc> = row.try_get("next_run_at").map_err(storage_error)?;
            if self
                .insert(&mut transaction, &schedule.occurrence(occurrence)?)
                .await?
                .created
            {
                created += 1;
            }
            sqlx::query("UPDATE schedules SET next_run_at = ?2, updated_at = ?3 WHERE name = ?1")
                .bind(schedule.name.as_str())
                .bind(schedule.recurrence.next_after(now))
                .bind(now)
                .execute(&mut *transaction)
                .await
                .map_err(storage_error)?;
        }
        transaction.commit().await.map_err(storage_error)?;
        Ok(created)
    }
}
