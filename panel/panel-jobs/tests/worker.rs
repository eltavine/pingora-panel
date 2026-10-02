#![forbid(unsafe_code)]

use async_trait::async_trait;
use chrono::{Duration as Span, Utc};
use panel_context::{IdempotencyKey, RequestId};
use panel_errors::{PanelError, Result};
use panel_jobs::{
    CancelOutcome, ClaimRequest, Finish, JobContext, JobHandler, JobId, JobKind, JobOrigin,
    JobSpec, JobState, JobStore, JobTemplate, MaintenanceWindow, MemoryJobStore, Recurrence,
    RetryPolicy, Schedule, ScheduleName, ScheduleStore, Worker, WorkerOptions,
};
use std::{
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio_util::sync::CancellationToken;

fn kind() -> JobKind {
    JobKind::new("test.job").unwrap()
}

fn spec(key: &str) -> JobSpec {
    let request = RequestId::new(format!("request-{key}")).unwrap();
    JobSpec::json(
        kind(),
        IdempotencyKey::new(key).unwrap(),
        &serde_json::json!({ "key": key }),
        JobOrigin {
            correlation_id: request.clone(),
            causation_id: request,
        },
    )
    .unwrap()
}

fn options() -> WorkerOptions {
    WorkerOptions {
        lease: Duration::from_millis(300),
        poll_interval: Duration::from_millis(10),
        retry: RetryPolicy::new(Duration::from_millis(10), Duration::from_millis(40)).unwrap(),
        shutdown_grace: Duration::from_secs(1),
        progress_interval: Duration::from_millis(20),
        ..WorkerOptions::new("worker-1")
    }
}

/// Fails `failures` times with `retryable`, then reports progress and
/// succeeds; or waits for cancellation when `blocking`.
struct Scripted {
    failures: u32,
    retryable: bool,
    blocking: bool,
    runs: AtomicU32,
}

impl Scripted {
    fn new(failures: u32, retryable: bool, blocking: bool) -> Arc<Self> {
        Arc::new(Self {
            failures,
            retryable,
            blocking,
            runs: AtomicU32::new(0),
        })
    }
}

#[async_trait]
impl JobHandler for Scripted {
    async fn run(&self, context: JobContext) -> Result<()> {
        let run = self.runs.fetch_add(1, Ordering::SeqCst) + 1;
        if self.blocking {
            context.report(10, "waiting")?;
            context.cancellation().cancelled().await;
            return Err(PanelError::internal("cancelled"));
        }
        if run <= self.failures {
            return Err(PanelError::unavailable("dependency down").retryable(self.retryable));
        }
        context.report(100, "done")?;
        Ok(())
    }
}

struct Running {
    shutdown: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl Running {
    fn start(
        store: &Arc<MemoryJobStore>,
        handler: Arc<dyn JobHandler>,
        options: WorkerOptions,
    ) -> Self {
        let shutdown = CancellationToken::new();
        let worker = Worker::new(Arc::clone(store) as Arc<dyn JobStore>, options)
            .with_handler(kind(), handler)
            .with_windows(Arc::clone(store) as Arc<dyn ScheduleStore>);
        let task = tokio::spawn(worker.run(shutdown.clone()));
        Self { shutdown, task }
    }

    async fn stop(self) {
        self.shutdown.cancel();
        self.task.await.unwrap();
    }
}

async fn settled(store: &MemoryJobStore, job: JobId) -> panel_jobs::Job {
    for _ in 0..500 {
        let current = store.get(job).await.unwrap().unwrap();
        if current.state.is_final() {
            return current;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("job {job} did not finish");
}

#[tokio::test]
async fn jobs_run_once_per_key_and_record_progress() {
    let store = Arc::new(MemoryJobStore::new());
    let first = store.enqueue(&spec("a")).await.unwrap();
    let again = store.enqueue(&spec("a")).await.unwrap();
    assert!(first.created && !again.created);
    assert_eq!(first.job_id, again.job_id);

    let handler = Scripted::new(0, true, false);
    let running = Running::start(&store, handler.clone(), options());
    let job = settled(&store, first.job_id).await;
    running.stop().await;
    assert_eq!(job.state, JobState::Succeeded);
    assert_eq!(job.attempts, 1);
    assert_eq!(job.progress.unwrap().percent(), 100);
    assert_eq!(handler.runs.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn retryable_failures_back_off_until_attempts_run_out() {
    let store = Arc::new(MemoryJobStore::new());
    let recovering = store.enqueue(&spec("recovering")).await.unwrap();
    let running = Running::start(&store, Scripted::new(2, true, false), options());
    let job = settled(&store, recovering.job_id).await;
    running.stop().await;
    assert_eq!(job.state, JobState::Succeeded);
    assert_eq!(job.attempts, 3);
    assert_eq!(
        job.last_error.unwrap().code,
        panel_errors::ErrorCode::UNAVAILABLE
    );

    let store = Arc::new(MemoryJobStore::new());
    let exhausted = store.enqueue(&spec("exhausted")).await.unwrap();
    let running = Running::start(&store, Scripted::new(10, true, false), options());
    let job = settled(&store, exhausted.job_id).await;
    running.stop().await;
    assert_eq!((job.state, job.attempts), (JobState::Failed, 3));

    let store = Arc::new(MemoryJobStore::new());
    let permanent = store.enqueue(&spec("permanent")).await.unwrap();
    let running = Running::start(&store, Scripted::new(10, false, false), options());
    let job = settled(&store, permanent.job_id).await;
    running.stop().await;
    assert_eq!((job.state, job.attempts), (JobState::Failed, 1));
}

#[tokio::test]
async fn cancellation_stops_queued_jobs_at_once_and_running_ones_cooperatively() {
    let store = Arc::new(MemoryJobStore::new());
    let queued = store.enqueue(&spec("queued")).await.unwrap();
    assert_eq!(
        store.cancel(queued.job_id).await.unwrap(),
        CancelOutcome::Cancelled
    );
    assert_eq!(
        store.cancel(queued.job_id).await.unwrap(),
        CancelOutcome::AlreadyFinished(JobState::Cancelled)
    );

    let blocking = store.enqueue(&spec("blocking")).await.unwrap();
    let handler = Scripted::new(0, true, true);
    let running = Running::start(&store, handler.clone(), options());
    while store.get(blocking.job_id).await.unwrap().unwrap().state != JobState::Running {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(
        store.cancel(blocking.job_id).await.unwrap(),
        CancelOutcome::Requested
    );
    let job = settled(&store, blocking.job_id).await;
    running.stop().await;
    assert_eq!(job.state, JobState::Cancelled);
    assert_eq!(job.progress.unwrap().message(), "waiting");
    assert_eq!(
        handler.runs.load(Ordering::SeqCst),
        1,
        "the cancelled job never ran"
    );
}

#[tokio::test]
async fn expired_leases_are_recovered_and_stale_leases_change_nothing() {
    let store = Arc::new(MemoryJobStore::new());
    let job = store.enqueue(&spec("orphaned")).await.unwrap();
    let kinds = [kind()];
    let crashed = store
        .claim(&ClaimRequest {
            owner: "crashed-worker",
            kinds: &kinds,
            limit: 1,
            lease: Duration::from_millis(20),
            open_windows: &[],
        })
        .await
        .unwrap()
        .remove(0);
    assert!(store
        .claim(&ClaimRequest {
            owner: "other",
            kinds: &kinds,
            limit: 1,
            lease: Duration::from_secs(5),
            open_windows: &[],
        })
        .await
        .unwrap()
        .is_empty());
    tokio::time::sleep(Duration::from_millis(40)).await;

    let running = Running::start(&store, Scripted::new(0, true, false), options());
    let finished = settled(&store, job.job_id).await;
    running.stop().await;
    assert_eq!(
        (finished.state, finished.attempts),
        (JobState::Succeeded, 2)
    );
    assert!(!store.finish(&crashed, Finish::Succeeded).await.unwrap());
}

#[tokio::test]
async fn window_bound_jobs_wait_for_an_open_window() {
    let store = Arc::new(MemoryJobStore::new());
    let now = Utc::now();
    let opened = |offset: Span| {
        let start = (now + offset).format("%Y%m%dT%H%M%SZ");
        Recurrence::parse(format!("DTSTART:{start}\nRRULE:FREQ=DAILY")).unwrap()
    };
    store
        .save_window(
            &MaintenanceWindow::new(
                ScheduleName::new("closed").unwrap(),
                opened(Span::hours(6)),
                Duration::from_secs(3600),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    store
        .save_window(
            &MaintenanceWindow::new(
                ScheduleName::new("open").unwrap(),
                opened(Span::minutes(-1)),
                Duration::from_secs(3600),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let held = store
        .enqueue(&JobSpec {
            maintenance_window: Some(ScheduleName::new("closed").unwrap()),
            ..spec("held")
        })
        .await
        .unwrap();
    let allowed = store
        .enqueue(&JobSpec {
            maintenance_window: Some(ScheduleName::new("open").unwrap()),
            ..spec("allowed")
        })
        .await
        .unwrap();
    let running = Running::start(&store, Scripted::new(0, true, false), options());
    assert_eq!(
        settled(&store, allowed.job_id).await.state,
        JobState::Succeeded
    );
    tokio::time::sleep(Duration::from_millis(100)).await;
    running.stop().await;
    assert_eq!(
        store.get(held.job_id).await.unwrap().unwrap().state,
        JobState::Queued
    );
}

#[tokio::test]
async fn schedules_enqueue_each_occurrence_exactly_once() {
    let store = MemoryJobStore::new();
    let start = Utc::now() - Span::minutes(10);
    let schedule = Schedule {
        name: ScheduleName::new("every-minute").unwrap(),
        recurrence: Recurrence::parse(format!(
            "DTSTART:{}\nRRULE:FREQ=MINUTELY",
            start.format("%Y%m%dT%H%M%SZ")
        ))
        .unwrap(),
        template: JobTemplate {
            kind: kind(),
            media_type: "application/json".into(),
            payload: b"{}".to_vec(),
            max_attempts: 1,
            priority: 0,
            maintenance_window: None,
        },
        enabled: true,
    };
    store
        .save_schedule(&schedule, start - Span::seconds(1))
        .await
        .unwrap();
    let now = Utc::now();
    assert_eq!(
        store.fire_due(now).await.unwrap(),
        1,
        "missed occurrences collapse"
    );
    assert_eq!(store.fire_due(now).await.unwrap(), 0);
    assert_eq!(store.fire_due(now + Span::minutes(1)).await.unwrap(), 1);
    assert_eq!(
        schedule.occurrence(start).unwrap().idempotency_key.as_str(),
        format!("schedule:every-minute:{}", start.format("%Y%m%dT%H%M%SZ"))
    );
}
