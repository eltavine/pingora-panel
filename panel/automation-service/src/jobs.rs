use async_trait::async_trait;
use chrono::{DateTime, Utc};
use panel_errors::{PanelError, Result};
use panel_events::{
    Actor, AggregateId, AggregateRef, AggregateType, EventDraft, EventEnvelope, EventOrigin,
    EventPayload, EventType, EventVersion, IdempotencyKey, Principal, RequestId, RequestScope,
    ServiceName,
};
use panel_jobs::{
    CancelOutcome, ClaimRequest, Enqueued, Finish, Job, JobError, JobId, JobKind, JobOrigin,
    JobSpec, JobState, JobStore, JobTemplate, Lease, MaintenanceWindow, Progress, Recurrence,
    Renewal, Schedule, ScheduleName, ScheduleStore,
};
use panel_postgres::{storage_error, PgOutbox, ServiceDatabase};
use serde_json::json;
use sqlx::{postgres::PgRow, PgConnection, PgPool, Row};
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

/// Matches a job only while `$1` is leased to owner `$2` for attempt `$3`.
macro_rules! lease_fence {
    () => {
        "job_id = $1 AND state = 'running' AND lease_owner = $2 AND attempts = $3 \
         AND lease_expires_at >= now()"
    };
}

/// Jobs, schedules and maintenance windows in the service schema.
///
/// Every state change and progress report appends a CloudEvent
/// (`automation.job.<change>.v1`) to the outbox in the same transaction, so
/// subscribers see each change exactly when it commits.
#[derive(Clone)]
pub struct PgJobStore {
    pool: PgPool,
    producer: ServiceName,
}

fn seconds(value: Duration) -> f64 {
    value.as_secs_f64()
}

fn count(value: u32) -> Result<i32> {
    i32::try_from(value).map_err(|_| PanelError::invalid_argument("count is too large"))
}

fn job(row: &PgRow) -> Result<Job> {
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

impl PgJobStore {
    pub fn new(database: &ServiceDatabase, producer: ServiceName) -> Self {
        Self {
            pool: database.pool().clone(),
            producer,
        }
    }

    fn event(&self, job: &Job, change: &str) -> Result<EventEnvelope> {
        let mut data = json!({
            "job_id": job.id.to_string(),
            "kind": job.kind.as_str(),
            "state": job.state.as_str(),
            "attempt": job.attempts,
            "max_attempts": job.max_attempts,
        });
        if let Some(progress) = &job.progress {
            data["progress"] =
                json!({ "percent": progress.percent(), "message": progress.message() });
        }
        if let Some(error) = &job.last_error {
            data["error"] = json!({ "code": error.code, "message": error.message });
        }
        let scope = RequestScope::new(job.origin.causation_id.clone())
            .with_correlation_id(job.origin.correlation_id.clone());
        Ok(EventEnvelope::new(
            EventDraft::new(
                EventType::new(format!("automation.job.{change}"))?,
                EventVersion::V1,
                AggregateRef::new(
                    AggregateType::new("job")?,
                    AggregateId::new(job.id.to_string())?,
                ),
                EventPayload::json(&data)?,
            ),
            EventOrigin::scoped(
                self.producer.clone(),
                &scope,
                Principal::system(Actor::new(self.producer.as_str())?),
            ),
            Utc::now(),
        ))
    }

    async fn publish(&self, connection: &mut PgConnection, job: &Job, change: &str) -> Result<()> {
        PgOutbox::append(connection, &self.event(job, change)?).await
    }

    async fn insert(&self, connection: &mut PgConnection, spec: &JobSpec) -> Result<Enqueued> {
        spec.validate()?;
        let inserted = sqlx::query(concat!(
            "INSERT INTO jobs (job_id, kind, idempotency_key, state, max_attempts, media_type, \
             payload, priority, run_after, maintenance_window, correlation_id, causation_id) \
             VALUES ($1, $2, $3, 'queued', $4, $5, $6, $7, greatest(coalesce($8, now()), now()), \
             $9, $10, $11) ON CONFLICT (kind, idempotency_key) DO NOTHING RETURNING ",
            columns!()
        ))
        .bind(JobId::generate().as_uuid())
        .bind(spec.kind.as_str())
        .bind(spec.idempotency_key.as_str())
        .bind(count(spec.max_attempts)?)
        .bind(&spec.media_type)
        .bind(&spec.payload)
        .bind(spec.priority)
        .bind(spec.not_before)
        .bind(spec.maintenance_window.as_ref().map(ScheduleName::as_str))
        .bind(spec.origin.correlation_id.as_str())
        .bind(spec.origin.causation_id.as_str())
        .fetch_optional(&mut *connection)
        .await
        .map_err(storage_error)?;
        if let Some(row) = inserted {
            let job = job(&row)?;
            self.publish(connection, &job, "queued").await?;
            return Ok(Enqueued {
                job_id: job.id,
                created: true,
            });
        }
        let existing: uuid::Uuid =
            sqlx::query_scalar("SELECT job_id FROM jobs WHERE kind = $1 AND idempotency_key = $2")
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
impl JobStore for PgJobStore {
    async fn enqueue(&self, spec: &JobSpec) -> Result<Enqueued> {
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
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
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        let settled = sqlx::query(concat!(
            "UPDATE jobs SET \
                 state = CASE WHEN cancel_requested THEN 'cancelled' ELSE 'failed' END, \
                 last_error_code = CASE WHEN cancel_requested THEN last_error_code \
                                        ELSE 'DEADLINE_EXCEEDED' END, \
                 last_error_message = CASE WHEN cancel_requested THEN last_error_message \
                                           ELSE 'the lease expired on the final attempt' END, \
                 lease_owner = NULL, lease_expires_at = NULL, \
                 finished_at = now(), updated_at = now() \
             WHERE state = 'running' AND lease_expires_at < now() \
               AND kind = ANY($1) AND (cancel_requested OR attempts >= max_attempts) \
             RETURNING ",
            columns!()
        ))
        .bind(&kinds)
        .fetch_all(&mut *transaction)
        .await
        .map_err(storage_error)?;
        for row in &settled {
            let job = job(row)?;
            let change = if job.state == JobState::Cancelled {
                "cancelled"
            } else {
                "failed"
            };
            self.publish(&mut transaction, &job, change).await?;
        }
        let claimed = sqlx::query(concat!(
            "UPDATE jobs SET state = 'running', attempts = attempts + 1, lease_owner = $1, \
                 lease_expires_at = now() + make_interval(secs => $2), updated_at = now() \
             WHERE job_id IN ( \
                 SELECT job_id FROM jobs \
                 WHERE kind = ANY($3) \
                   AND (maintenance_window IS NULL OR maintenance_window = ANY($4)) \
                   AND ((state IN ('queued', 'retrying') AND run_after <= now() \
                         AND NOT cancel_requested) \
                        OR (state = 'running' AND lease_expires_at < now())) \
                 ORDER BY priority DESC, run_after, job_id \
                 LIMIT $5 FOR UPDATE SKIP LOCKED) \
             RETURNING ",
            columns!()
        ))
        .bind(request.owner)
        .bind(seconds(request.lease))
        .bind(&kinds)
        .bind(&windows)
        .bind(i64::try_from(request.limit).unwrap_or(i64::MAX))
        .fetch_all(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let mut leases = Vec::with_capacity(claimed.len());
        for row in &claimed {
            let job = job(row)?;
            self.publish(&mut transaction, &job, "started").await?;
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
        let renewed: Option<bool> = sqlx::query_scalar(concat!(
            "UPDATE jobs SET lease_expires_at = now() + make_interval(secs => $4), \
                 updated_at = now() WHERE ",
            lease_fence!(),
            " RETURNING cancel_requested"
        ))
        .bind(lease.job.id.as_uuid())
        .bind(&lease.owner)
        .bind(count(lease.attempt)?)
        .bind(seconds(extend_by))
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?;
        Ok(match renewed {
            Some(cancel_requested) => Renewal::Held { cancel_requested },
            None => Renewal::Lost,
        })
    }

    async fn report_progress(&self, lease: &Lease, progress: &Progress) -> Result<bool> {
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        let updated = sqlx::query(concat!(
            "UPDATE jobs SET progress_percent = $4, progress_message = $5, updated_at = now() \
             WHERE ",
            lease_fence!(),
            " RETURNING ",
            columns!()
        ))
        .bind(lease.job.id.as_uuid())
        .bind(&lease.owner)
        .bind(count(lease.attempt)?)
        .bind(i16::from(progress.percent()))
        .bind(progress.message())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(storage_error)?;
        let Some(row) = updated else {
            return Ok(false);
        };
        self.publish(&mut transaction, &job(&row)?, "progressed")
            .await?;
        transaction.commit().await.map_err(storage_error)?;
        Ok(true)
    }

    async fn finish(&self, lease: &Lease, finish: Finish) -> Result<bool> {
        let (state, change, run_after, error) = match &finish {
            Finish::Succeeded => ("succeeded", "succeeded", None, None),
            Finish::Cancelled => ("cancelled", "cancelled", None, None),
            Finish::Retry { at, error } => ("retrying", "retrying", Some(*at), Some(error)),
            Finish::Failed { error } => ("failed", "failed", None, Some(error)),
            _ => return Err(PanelError::unsupported_capability("unknown job outcome")),
        };
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        let updated = sqlx::query(concat!(
            "UPDATE jobs SET state = $4, lease_owner = NULL, lease_expires_at = NULL, \
                 run_after = coalesce($5, run_after), \
                 last_error_code = coalesce($6, last_error_code), \
                 last_error_message = coalesce($7, last_error_message), \
                 finished_at = CASE WHEN $4 IN ('succeeded', 'failed', 'cancelled') \
                                    THEN now() END, \
                 updated_at = now() \
             WHERE ",
            lease_fence!(),
            " RETURNING ",
            columns!()
        ))
        .bind(lease.job.id.as_uuid())
        .bind(&lease.owner)
        .bind(count(lease.attempt)?)
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
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        let state: Option<String> =
            sqlx::query_scalar("SELECT state FROM jobs WHERE job_id = $1 FOR UPDATE")
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
                    "UPDATE jobs SET state = 'cancelled', cancel_requested = true, \
                         finished_at = now(), updated_at = now() \
                     WHERE job_id = $1 RETURNING ",
                    columns!()
                ))
                .bind(job_id.as_uuid())
                .fetch_one(&mut *transaction)
                .await
                .map_err(storage_error)?;
                self.publish(&mut transaction, &job(&row)?, "cancelled")
                    .await?;
                CancelOutcome::Cancelled
            }
            JobState::Running => {
                sqlx::query(
                    "UPDATE jobs SET cancel_requested = true, updated_at = now() WHERE job_id = $1",
                )
                .bind(job_id.as_uuid())
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
            " FROM jobs WHERE job_id = $1"
        ))
        .bind(job_id.as_uuid())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage_error)?
        .as_ref()
        .map(job)
        .transpose()
    }
}

fn schedule(row: &PgRow) -> Result<Schedule> {
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
impl ScheduleStore for PgJobStore {
    async fn save_schedule(&self, schedule: &Schedule, now: DateTime<Utc>) -> Result<()> {
        sqlx::query(
            "INSERT INTO schedules (name, recurrence, kind, media_type, payload, max_attempts, \
                 priority, maintenance_window, enabled, next_run_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
             ON CONFLICT (name) DO UPDATE SET recurrence = EXCLUDED.recurrence, \
                 kind = EXCLUDED.kind, media_type = EXCLUDED.media_type, \
                 payload = EXCLUDED.payload, max_attempts = EXCLUDED.max_attempts, \
                 priority = EXCLUDED.priority, maintenance_window = EXCLUDED.maintenance_window, \
                 enabled = EXCLUDED.enabled, next_run_at = EXCLUDED.next_run_at, \
                 updated_at = now()",
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
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        Ok(())
    }

    async fn save_window(&self, window: &MaintenanceWindow) -> Result<()> {
        sqlx::query(
            "INSERT INTO maintenance_windows (name, recurrence, duration_seconds) \
             VALUES ($1, $2, $3) ON CONFLICT (name) DO UPDATE \
             SET recurrence = EXCLUDED.recurrence, duration_seconds = EXCLUDED.duration_seconds, \
                 updated_at = now()",
        )
        .bind(window.name().as_str())
        .bind(window.recurrence().as_str())
        .bind(i32::try_from(window.duration().as_secs()).unwrap_or(i32::MAX))
        .execute(&self.pool)
        .await
        .map_err(storage_error)?;
        Ok(())
    }

    async fn windows(&self) -> Result<Vec<MaintenanceWindow>> {
        let rows: Vec<(String, String, i32)> = sqlx::query_as(
            "SELECT name, recurrence, duration_seconds FROM maintenance_windows ORDER BY name",
        )
        .fetch_all(&self.pool)
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
        let mut transaction = self.pool.begin().await.map_err(storage_error)?;
        let due = sqlx::query(
            "SELECT name, recurrence, kind, media_type, payload, max_attempts, priority, \
                    maintenance_window, enabled, next_run_at \
             FROM schedules WHERE enabled AND next_run_at <= $1 \
             ORDER BY next_run_at FOR UPDATE SKIP LOCKED",
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
            sqlx::query(
                "UPDATE schedules SET next_run_at = $2, updated_at = now() WHERE name = $1",
            )
            .bind(schedule.name.as_str())
            .bind(schedule.recurrence.next_after(now))
            .execute(&mut *transaction)
            .await
            .map_err(storage_error)?;
        }
        transaction.commit().await.map_err(storage_error)?;
        Ok(created)
    }
}
